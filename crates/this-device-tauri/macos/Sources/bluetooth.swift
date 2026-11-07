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
