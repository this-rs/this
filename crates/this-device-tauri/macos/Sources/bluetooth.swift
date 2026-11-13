// One Bluetooth scan of a fixed duration on macOS (CoreBluetooth, central role).
//
// Flow: create the central manager (macOS asks for Bluetooth permission the first time),
// wait for `poweredOn`, scan for `duration_ms`, answer with the devices seen, deduplicated
// by identifier. Requires NSBluetoothAlwaysUsageDescription in Info.plist.
//
// Blocking: the scan blocks the calling thread via DispatchSemaphore. The Rust side calls
// from spawn_blocking.
//
// CoreBluetooth is the same on macOS and iOS: CBCentralManager, CBCentralManagerDelegate,
// CBPeripheral, CBUUID, CBManagerAuthorization all work identically.

import CoreBluetooth
import Foundation

/// How long to wait for the radio to report `poweredOn` before giving up.
private let readyTimeoutSeconds: Double = 5

/// Scan duration bounds (milliseconds), same as the iOS side.
private let scanMsMin: Int = 1000
private let scanMsMax: Int = 10000

// MARK: - Public API (called from dispatch.swift)

enum MacBluetooth {
    /// The Bluetooth permission state, in contract words.
    static func currentState() -> String {
        if #available(macOS 10.15, *) {
            switch CBManager.authorization {
            case .allowedAlways:
                return "granted"
            case .denied, .restricted:
                return "denied"
            case .notDetermined:
                return "prompt"
            @unknown default:
                return "prompt"
            }
        }
        // Before macOS 10.15, Bluetooth had no permission model.
        return "granted"
    }

    /// One-shot BLE scan. Args: `{"duration_ms": 3000, "services": ["180d"]}`.
    /// Blocks until the scan duration elapses or an error occurs.
    static func scan(_ args: [String: Any]) -> String {
        // Read authorization without prompting: denied cannot be scanned anyway.
        if #available(macOS 10.15, *) {
            let auth = CBManager.authorization
            if auth == .denied || auth == .restricted {
                return errorReply(code: ReplyCode.notAuthorized)
            }
        }

        let durationMs = (args["duration_ms"] as? Int) ?? 3000
        let clamped = max(scanMsMin, min(scanMsMax, durationMs))
        let serviceStrings = args["services"] as? [String]
        let uuids: [CBUUID]? = serviceStrings.flatMap { list in
            let mapped = list.compactMap { str -> CBUUID? in
                // CBUUID(string:) can crash on invalid input, so validate first.
                guard !str.isEmpty else { return nil }
                return CBUUID(string: str)
            }
            return mapped.isEmpty ? nil : mapped
        }

        let scanner = BluetoothScanner(durationMs: clamped, services: uuids)
        return scanner.run()
    }
}

// MARK: - One-shot scanner (blocking with delegate)

/// A Bluetooth device seen during a scan, for JSON encoding.
private struct ScannedDevice {
    let id: String
    let name: String?
    let rssi: Int
}

private final class BluetoothScanner: NSObject, CBCentralManagerDelegate {
    private let durationSeconds: Double
    private let services: [CBUUID]?
    private let semaphore = DispatchSemaphore(value: 0)

    private var central: CBCentralManager?
    private var scanning = false
    private var finished = false

    // Devices seen, deduplicated by identifier, in discovery order.
    private var order: [String] = []
    private var found: [String: ScannedDevice] = [:]

    private var result: String = errorReply(code: ReplyCode.timeout)

    init(durationMs: Int, services: [CBUUID]?) {
        self.durationSeconds = Double(durationMs) / 1000
        self.services = services
        super.init()
    }

    /// Blocks until the scan completes, errors, or times out.
    func run() -> String {
        // CBCentralManager must be created on a thread with a run loop for delegate callbacks.
        // We create it on the main thread and block this thread.
        DispatchQueue.main.async { [self] in
            central = CBCentralManager(delegate: self, queue: .main)

            // Timeout for poweredOn: if the radio never reports ready, we give up.
            DispatchQueue.main.asyncAfter(deadline: .now() + readyTimeoutSeconds) { [weak self] in
                guard let self = self, !self.scanning, !self.finished else { return }
                self.finish(errorReply(code: ReplyCode.timeout))
            }
        }

        // Block until the scan finishes (the semaphore is signalled by finish()).
        let totalTimeout = readyTimeoutSeconds + durationSeconds + 2
        _ = semaphore.wait(timeout: .now() + totalTimeout)

        // Clean up on the main thread.
        DispatchQueue.main.sync { [self] in
            if scanning { central?.stopScan() }
            central?.delegate = nil
            central = nil
        }

        return result
    }

    // MARK: CBCentralManagerDelegate

    func centralManagerDidUpdateState(_ central: CBCentralManager) {
        switch central.state {
        case .poweredOn:
            guard !scanning, !finished else { return }
            scanning = true
            central.scanForPeripherals(
                withServices: services,
                options: [CBCentralManagerScanOptionAllowDuplicatesKey: true])
            DispatchQueue.main.asyncAfter(deadline: .now() + durationSeconds) { [weak self] in
                guard let self = self, !self.finished else { return }
                self.finish(self.devicesJson())
            }
        case .unauthorized:
            finish(errorReply(code: ReplyCode.notAuthorized))
        case .poweredOff, .unsupported:
            finish(errorReply(code: ReplyCode.unavailable))
        case .unknown, .resetting:
            // Transient: the manager reports the real state next. The ready timeout covers stalls.
            break
        @unknown default:
            finish(errorReply(code: ReplyCode.unavailable))
        }
    }

    func centralManager(
        _ central: CBCentralManager,
        didDiscover peripheral: CBPeripheral,
        advertisementData: [String: Any],
        rssi RSSI: NSNumber
    ) {
        // 127 is CoreBluetooth's "no reading" marker: keep the last real value.
        let id = peripheral.identifier.uuidString
        let name = peripheral.name ?? (advertisementData[CBAdvertisementDataLocalNameKey] as? String)
        let rssiValue = RSSI.intValue == 127 ? found[id]?.rssi : RSSI.intValue
        guard let rssi = rssiValue else { return }
        if found[id] == nil { order.append(id) }
        // A name seen later is kept; an earlier one is not erased by a nameless advertisement.
        let knownName = found[id]?.name
        found[id] = ScannedDevice(id: id, name: name ?? knownName, rssi: rssi)
    }

    // MARK: Finish

    private func devicesJson() -> String {
        let devices: [[String: Any]] = order.compactMap { id in
            guard let dev = found[id] else { return nil }
            var dict: [String: Any] = ["id": dev.id, "rssi": dev.rssi]
            // Explicit null for nil name (serde expects it).
            dict["name"] = dev.name as Any? ?? NSNull()
            return dict
        }
        return encodeDictJson(["kind": "devices", "devices": devices])
    }

    private func finish(_ json: String) {
        guard !finished else { return }
        finished = true
        result = json
        semaphore.signal()
    }
}

// MARK: - Connection engine (central role)
//
// A port of `ios/Sources/BluetoothCentral.swift`, adapted to the macOS blocking bridge: every
// command blocks on a semaphore until the engine answers, and the wire shapes match
// `families/bluetooth.rs` (`BluetoothReply`) exactly. The central manager is shared and kept
// for the life of the app, because a `CBPeripheral` stays valid only while its manager lives.

/// A non-empty string argument.
private func btString(_ value: Any?) -> String? {
    guard let string = value as? String else { return nil }
    let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
    return trimmed.isEmpty ? nil : trimmed
}

private struct BtStateAnswer: Encodable {
    let kind = "state"
    let state: String
}

private struct BtDeviceAnswer: Encodable {
    let id: String
    let name: String?
    let state: String

    private enum Key: String, CodingKey {
        case kind, id, name, state
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Key.self)
        try c.encode("device", forKey: .kind)
        try c.encode(id, forKey: .id)
        // `encode` (not `encodeIfPresent`): a peripheral without a name answers `null`, and the
        // Rust side reads `None` rather than a missing field.
        try c.encode(name, forKey: .name)
        try c.encode(state, forKey: .state)
    }
}

private struct BtCharacteristicInfo: Encodable {
    let uuid: String
    let properties: [String]
}

private struct BtServiceInfo: Encodable {
    let uuid: String
    let characteristics: [BtCharacteristicInfo]
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

private struct BtNotification: Encodable {
    let characteristic: String
    let data_base64: String
    let ts_ms: Int64
}

private struct BtNotificationsAnswer: Encodable {
    let kind = "notifications"
    let notifications: [BtNotification]
}

/// The Bluetooth central, shared by every call. The result of an operation is either a JSON
/// string (already encoded) or an error code, exactly as the family's contract names it.
private final class MacBluetoothCentral: NSObject, CBCentralManagerDelegate, CBPeripheralDelegate {
    static let shared = MacBluetoothCentral()

    /// The bound of an operation that is not a connection.
    static let defaultTimeoutSeconds: Double = 10
    /// How many values the engine keeps per subscribed characteristic.
    static let ringMax = 32
    /// How long to wait for the radio to report a state before giving up.
    static let readyTimeoutSeconds: Double = 5

    private struct Op {
        let done: (Result<String, WireError>) -> Void
    }

    private var manager: CBCentralManager?
    private var stateWaiters: [(CBCentralManager) -> Void] = []
    private var ops: [String: Op] = [:]
    private var characteristics: [String: CBCharacteristic] = [:]
    private var rings: [String: [BtNotification]] = [:]
    private var connected: Set<String> = []

    // ---- The manager -------------------------------------------------------------------

    /// The manager, created on first use. Creating it is what makes macOS ask for the
    /// permission the first time.
    private func central() -> CBCentralManager {
        if let manager = manager { return manager }
        let created = CBCentralManager(delegate: self, queue: .main)
        manager = created
        return created
    }

    /// Run `body` once the radio reports `poweredOn`, or answer a failure now. All engine state
    /// is touched on the main thread (the caller enters through `MacBluetooth` on main).
    private func whenReady(
        done: @escaping (Result<String, WireError>) -> Void,
        body: @escaping (CBCentralManager) -> Void
    ) {
        let manager = central()
        switch manager.state {
        case .poweredOn:
            body(manager)
        case .unauthorized:
            done(.failure(WireError(code: "not_authorized")))
        case .poweredOff, .unsupported:
            done(.failure(WireError(code: "unavailable")))
        case .unknown, .resetting:
            // Transient: the delegate reports the real state next. The ready timeout covers a
            // stall, and the caller's blocking wrapper ignores a second answer.
            stateWaiters.append { ready in body(ready) }
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.readyTimeoutSeconds) {
                done(.failure(WireError(code: "timeout")))
            }
        @unknown default:
            done(.failure(WireError(code: "unavailable")))
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

    func radioState(_ done: @escaping (Result<String, WireError>) -> Void) {
        let manager = central()
        if manager.state == .unknown {
            stateWaiters.append { ready in
                done(.success(encodeJson(BtStateAnswer(state: Self.name(ready.state)))))
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.readyTimeoutSeconds) {
                done(.success(encodeJson(BtStateAnswer(state: "unknown"))))
            }
            return
        }
        done(.success(encodeJson(BtStateAnswer(state: Self.name(manager.state)))))
    }

    // ---- Operations --------------------------------------------------------------------

    private func key(_ op: String, _ peer: String, _ characteristic: String? = nil) -> String {
        if let characteristic = characteristic {
            return "\(op):\(peer):\(characteristic)"
        }
        return "\(op):\(peer)"
    }

    private func begin(
        _ name: String,
        timeoutSeconds: Double = MacBluetoothCentral.defaultTimeoutSeconds,
        done: @escaping (Result<String, WireError>) -> Void
    ) {
        ops[name] = Op(done: done)
        DispatchQueue.main.asyncAfter(deadline: .now() + timeoutSeconds) { [weak self] in
            self?.finish(name, .failure(WireError(code: "timeout")))
        }
    }

    private func finish(_ name: String, _ result: Result<String, WireError>) {
        guard let op = ops.removeValue(forKey: name) else { return }
        op.done(result)
    }

    private func failAll(_ code: String) {
        let waiting = ops
        ops.removeAll()
        for (_, op) in waiting {
            op.done(.failure(WireError(code: code)))
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
        done: @escaping (Result<String, WireError>) -> Void
    ) {
        let id = id.uppercased()
        whenReady(done: done) { [weak self] manager in
            guard let self = self else { return }
            guard let peer = self.peripheral(id: id, manager) else {
                done(.failure(WireError(code: "not_supported")))
                return
            }
            if peer.state == .connected {
                self.connected.insert(id)
                done(.success(encodeJson(BtDeviceAnswer(id: id, name: peer.name, state: "connected"))))
                return
            }
            let name = self.key("connect", id)
            self.begin(name, timeoutSeconds: timeoutSeconds, done: done)
            peer.delegate = self
            manager.connect(peer, options: nil)
        }
    }

    func disconnect(id: String, done: @escaping (Result<String, WireError>) -> Void) {
        let id = id.uppercased()
        whenReady(done: done) { [weak self] manager in
            guard let self = self else { return }
            guard let peer = self.peripheral(id: id, manager) else {
                done(.failure(WireError(code: "not_supported")))
                return
            }
            let name = self.key("disconnect", id)
            self.begin(name, done: done)
            if peer.state == .disconnected {
                self.connected.remove(id)
                self.finish(
                    name,
                    .success(encodeJson(BtDeviceAnswer(id: id, name: peer.name, state: "disconnected")))
                )
                return
            }
            manager.cancelPeripheralConnection(peer)
        }
    }

    func services(id: String, done: @escaping (Result<String, WireError>) -> Void) {
        let id = id.uppercased()
        whenReady(done: done) { [weak self] manager in
            guard let self = self else { return }
            guard let peer = self.peripheral(id: id, manager) else {
                done(.failure(WireError(code: "not_supported")))
                return
            }
            guard self.connected.contains(id) else {
                done(.failure(WireError(code: "unavailable")))
                return
            }
            let name = self.key("services", id)
            self.begin(name, done: done)
            peer.delegate = self
            peer.discoverServices(nil)
        }
    }

    func read(
        id: String,
        characteristic: String,
        done: @escaping (Result<String, WireError>) -> Void
    ) {
        let id = id.uppercased()
        let characteristic = characteristic.uppercased()
        whenReady(done: done) { [weak self] manager in
            guard let self = self else { return }
            guard let peer = self.peripheral(id: id, manager) else {
                done(.failure(WireError(code: "not_supported")))
                return
            }
            guard let target = self.characteristics["\(id)/\(characteristic)"] else {
                done(.failure(WireError(code: "not_supported")))
                return
            }
            let name = self.key("read", id, characteristic)
            self.begin(name, done: done)
            peer.readValue(for: target)
        }
    }

    func write(
        id: String,
        characteristic: String,
        data: Data,
        withResponse: Bool,
        done: @escaping (Result<String, WireError>) -> Void
    ) {
        let id = id.uppercased()
        let characteristic = characteristic.uppercased()
        whenReady(done: done) { [weak self] manager in
            guard let self = self else { return }
            guard let peer = self.peripheral(id: id, manager) else {
                done(.failure(WireError(code: "not_supported")))
                return
            }
            guard let target = self.characteristics["\(id)/\(characteristic)"] else {
                done(.failure(WireError(code: "not_supported")))
                return
            }
            let type: CBCharacteristicWriteType = withResponse ? .withResponse : .withoutResponse
            if !withResponse {
                // No callback for a write without response: the queue accepting it is the whole
                // confirmation.
                peer.writeValue(data, for: target, type: type)
                done(.success(encodeJson(BtWrittenAnswer(bytes: data.count))))
                return
            }
            let name = self.key("write", id, characteristic)
            self.begin(name, done: done)
            peer.writeValue(data, for: target, type: type)
        }
    }

    func subscribe(
        id: String,
        characteristic: String,
        done: @escaping (Result<String, WireError>) -> Void
    ) {
        let id = id.uppercased()
        let characteristic = characteristic.uppercased()
        whenReady(done: done) { [weak self] manager in
            guard let self = self else { return }
            guard let peer = self.peripheral(id: id, manager) else {
                done(.failure(WireError(code: "not_supported")))
                return
            }
            guard let target = self.characteristics["\(id)/\(characteristic)"] else {
                done(.failure(WireError(code: "not_supported")))
                return
            }
            let name = self.key("subscribe", id, characteristic)
            self.begin(name, done: done)
            if target.properties.contains(.notify) || target.properties.contains(.indicate) {
                peer.setNotifyValue(true, for: target)
            } else {
                self.finish(name, .failure(WireError(code: "not_supported")))
            }
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

// MARK: - CBCentralManagerDelegate / CBPeripheralDelegate

extension MacBluetoothCentral {
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
            stateWaiters = waiters
        @unknown default:
            failAll("unavailable")
        }
    }

    func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        let id = peripheral.identifier.uuidString
        connected.insert(id)
        peripheral.delegate = self
        finish(
            key("connect", id),
            .success(encodeJson(BtDeviceAnswer(id: id, name: peripheral.name, state: "connected")))
        )
    }

    func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        let id = peripheral.identifier.uuidString
        finish(key("connect", id), .failure(WireError(code: "unavailable")))
    }

    func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {
        let id = peripheral.identifier.uuidString
        connected.remove(id)
        for key in characteristics.keys where key.hasPrefix("\(id)/") {
            characteristics.removeValue(forKey: key)
        }
        finish(
            key("disconnect", id),
            .success(encodeJson(BtDeviceAnswer(id: id, name: peripheral.name, state: "disconnected")))
        )
    }

    func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        let id = peripheral.identifier.uuidString
        if error != nil {
            finish(key("services", id), .failure(WireError(code: "internal")))
            return
        }
        let services = peripheral.services ?? []
        if services.isEmpty {
            finish(key("services", id), .success(encodeJson(BtServicesAnswer(services: []))))
            return
        }
        for service in services {
            peripheral.discoverCharacteristics(nil, for: service)
        }
    }

    func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        let id = peripheral.identifier.uuidString
        if error != nil {
            finish(key("services", id), .failure(WireError(code: "internal")))
            return
        }
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
        finish(key("services", id), .success(encodeJson(BtServicesAnswer(services: answer))))
    }

    func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        let id = peripheral.identifier.uuidString
        let uuid = characteristic.uuid.uuidString
        let data = characteristic.value ?? Data()
        if error != nil {
            finish(key("read", id, uuid), .failure(WireError(code: "internal")))
            return
        }
        finish(
            key("read", id, uuid),
            .success(encodeJson(BtValueAnswer(characteristic: uuid, data_base64: data.base64EncodedString())))
        )
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

    func peripheral(_ peripheral: CBPeripheral, didWriteValueFor characteristic: CBCharacteristic, error: Error?) {
        let id = peripheral.identifier.uuidString
        let uuid = characteristic.uuid.uuidString
        if error != nil {
            finish(key("write", id, uuid), .failure(WireError(code: "internal")))
            return
        }
        finish(
            key("write", id, uuid),
            .success(encodeJson(BtWrittenAnswer(bytes: characteristic.value?.count ?? 0)))
        )
    }

    func peripheral(_ peripheral: CBPeripheral, didUpdateNotificationStateFor characteristic: CBCharacteristic, error: Error?) {
        let id = peripheral.identifier.uuidString
        let uuid = characteristic.uuid.uuidString
        if error != nil {
            finish(key("subscribe", id, uuid), .failure(WireError(code: "not_supported")))
            return
        }
        finish(
            key("subscribe", id, uuid),
            .success(encodeJson(BtSubscribedAnswer(characteristic: uuid)))
        )
    }
}

// MARK: - Connection commands (called from dispatch.swift)

extension MacBluetooth {
    /// `bluetoothState` — the radio's own state, distinct from the permission.
    static func radioState() -> String {
        return onMainBlocking(timeout: MacBluetoothCentral.readyTimeoutSeconds + 1) { done in
            MacBluetoothCentral.shared.radioState(done)
        }
    }

    /// `bluetoothConnect` — Args: `{"id": "<uuid>", "timeout_ms": 10000}`.
    static func connect(_ args: [String: Any]) -> String {
        guard let id = btString(args["id"]) else {
            return errorReply(code: "missing_argument")
        }
        let timeoutMs = (args["timeout_ms"] as? Int) ?? 10_000
        let timeoutSeconds = Double(min(max(timeoutMs, 1_000), 15_000)) / 1000
        return onMainBlocking(timeout: timeoutSeconds + 2) { done in
            MacBluetoothCentral.shared.connect(id: id, timeoutSeconds: timeoutSeconds, done: done)
        }
    }

    /// `bluetoothDisconnect` — Args: `{"id": "<uuid>"}`.
    static func disconnect(_ args: [String: Any]) -> String {
        guard let id = btString(args["id"]) else {
            return errorReply(code: "missing_argument")
        }
        return onMainBlocking(timeout: MacBluetoothCentral.defaultTimeoutSeconds + 2) { done in
            MacBluetoothCentral.shared.disconnect(id: id, done: done)
        }
    }

    /// `bluetoothServices` — Args: `{"id": "<uuid>"}`.
    static func services(_ args: [String: Any]) -> String {
        guard let id = btString(args["id"]) else {
            return errorReply(code: "missing_argument")
        }
        return onMainBlocking(timeout: MacBluetoothCentral.defaultTimeoutSeconds + 2) { done in
            MacBluetoothCentral.shared.services(id: id, done: done)
        }
    }

    /// `bluetoothRead` — Args: `{"id": "<uuid>", "characteristic": "2A37"}`.
    static func read(_ args: [String: Any]) -> String {
        guard let id = btString(args["id"]),
              let characteristic = btString(args["characteristic"]) else {
            return errorReply(code: "missing_argument")
        }
        return onMainBlocking(timeout: MacBluetoothCentral.defaultTimeoutSeconds + 2) { done in
            MacBluetoothCentral.shared.read(id: id, characteristic: characteristic, done: done)
        }
    }

    /// `bluetoothWrite` — Args: `{"id": ..., "characteristic": ..., "data_base64": ..., "with_response": true}`.
    static func write(_ args: [String: Any]) -> String {
        guard let id = btString(args["id"]),
              let characteristic = btString(args["characteristic"]),
              let base64 = btString(args["data_base64"]) else {
            return errorReply(code: "missing_argument")
        }
        guard let data = Data(base64Encoded: base64) else {
            return errorReply(code: "bad_base64")
        }
        let withResponse = (args["with_response"] as? Bool) ?? true
        return onMainBlocking(timeout: MacBluetoothCentral.defaultTimeoutSeconds + 2) { done in
            MacBluetoothCentral.shared.write(
                id: id, characteristic: characteristic, data: data, withResponse: withResponse, done: done)
        }
    }

    /// `bluetoothSubscribe` — Args: `{"id": ..., "characteristic": ...}`.
    static func subscribe(_ args: [String: Any]) -> String {
        guard let id = btString(args["id"]),
              let characteristic = btString(args["characteristic"]) else {
            return errorReply(code: "missing_argument")
        }
        return onMainBlocking(timeout: MacBluetoothCentral.defaultTimeoutSeconds + 2) { done in
            MacBluetoothCentral.shared.subscribe(id: id, characteristic: characteristic, done: done)
        }
    }

    /// `bluetoothNotifications` — drain the buffer of a subscribed characteristic.
    static func notifications(_ args: [String: Any]) -> String {
        guard let id = btString(args["id"]),
              let characteristic = btString(args["characteristic"]) else {
            return errorReply(code: "missing_argument")
        }
        return onMainBlocking(timeout: MacBluetoothCentral.defaultTimeoutSeconds) { done in
            let values = MacBluetoothCentral.shared.notifications(id: id, characteristic: characteristic)
            done(.success(encodeJson(BtNotificationsAnswer(notifications: values))))
        }
    }
}
