// Local network discovery on macOS (Network framework, NWBrowser / Bonjour).
//
// macOS does NOT have the local network permission dialog that iOS 14+ shows. Any app
// can browse Bonjour services freely. So the status is always "granted".
//
// Browse: one NWBrowser on one service type from the closed list, for 5 seconds.
// The answer carries the names and types of the services found, nothing else: no endpoint
// is resolved, so no IP address leaves this file.

import Foundation
import Network

enum MacLocalNetwork {
    /// The only service types a browse may name. Mirrors the iOS side and families/local_network.rs.
    static let allowedTypes: [String] = ["_http._tcp", "_atelier._tcp"]
    /// The only types an announcement may use: the Atelier's own, never `_http._tcp`.
    /// Mirrors `ADVERTISE_TYPES` in families/local_network.rs.
    static let advertisableTypes: [String] = ["_atelier._tcp"]
    /// How long the browse listens.
    static let browseSeconds: TimeInterval = 5

    // MARK: - Permission state

    /// macOS has no local network permission: always granted.
    static func currentState() -> String {
        return Etat.granted.fil
    }

    // MARK: - Commands

    /// Status: always granted on macOS (no permission dialog).
    static func status() -> String {
        return statusReply(state: Etat.granted.fil)
    }

    /// One Bonjour browse of a type in the closed list, for a few seconds.
    /// Returns the services found (name, service_type).
    static func browse(_ args: [String: Any]) -> String {
        let serviceType = (args["service_type"] as? String) ?? "_http._tcp"
        guard allowedTypes.contains(serviceType) else {
            return errorReply(code: "unsupported")
        }

        return blockingCall(timeout: browseSeconds + 2) { done in
            let browser = NWBrowser(
                for: .bonjour(type: serviceType, domain: nil),
                using: NWParameters()
            )

            var foundResults: Set<NWBrowser.Result> = []
            let lock = NSLock()

            browser.browseResultsChangedHandler = { results, _ in
                lock.lock()
                foundResults = results
                lock.unlock()
            }

            browser.stateUpdateHandler = { state in
                switch state {
                case .failed(let error):
                    browser.cancel()
                    done(errorReply(code: ReplyCode.unavailable))
                    _ = error  // Logged for debugging, not exposed.
                default:
                    break
                }
            }

            browser.start(queue: .global(qos: .userInitiated))

            // Wait the browse window, then collect results.
            DispatchQueue.global(qos: .userInitiated).asyncAfter(
                deadline: .now() + browseSeconds
            ) {
                browser.cancel()

                lock.lock()
                let snapshot = foundResults
                lock.unlock()

                // The family's wire shape (families/local_network.rs): name + service_type,
                // nothing else. The old `devices`/`type`/`domain` shape did not parse there.
                let services: [[String: Any]] = snapshot.compactMap { result in
                    guard case let .service(name, svcType, _, _) = result.endpoint else {
                        return nil
                    }
                    return ["name": name, "service_type": svcType]
                }
                .sorted {
                    (($0["name"] as? String ?? ""), ($0["service_type"] as? String ?? ""))
                        < (($1["name"] as? String ?? ""), ($1["service_type"] as? String ?? ""))
                }

                done(encodeDictJson([
                    "kind": "services",
                    "services": services
                ]))
            }
        }
    }
}

// MARK: - Resolve and advertise (NetService)
//
// `NetService` needs a run loop, and `macos_device_call` may run on any thread (the main one
// included, where a semaphore wait would deadlock the run loop). So every NetService lives on one
// dedicated thread with its own run loop, and the caller blocks on a semaphore as the other
// commands do.

/// One thread, one run loop, for every NetService of this file.
final class NetServiceLoop {
    static let shared = NetServiceLoop()

    private var runLoop: CFRunLoop?
    private let ready = DispatchSemaphore(value: 0)

    private init() {
        let thread = Thread { [self] in
            runLoop = CFRunLoopGetCurrent()
            // A source keeps the run loop alive while nothing is scheduled.
            var context = CFRunLoopSourceContext()
            let source = CFRunLoopSourceCreate(nil, 0, &context)
            CFRunLoopAddSource(runLoop, source, .defaultMode)
            ready.signal()
            CFRunLoopRun()
        }
        thread.name = "this-device.netservice"
        thread.start()
        ready.wait()
    }

    func perform(_ block: @escaping () -> Void) {
        guard let rl = runLoop else { return }
        CFRunLoopPerformBlock(rl, CFRunLoopMode.defaultMode.rawValue, block)
        CFRunLoopWakeUp(rl)
    }
}

/// A resolution in flight, kept until it answers.
final class MacResolve: NSObject, NetServiceDelegate {
    private let service: NetService
    private let done: (String) -> Void
    private var finished = false
    // Only touched on the NetService thread.
    static var active: [ObjectIdentifier: MacResolve] = [:]

    init(service: NetService, done: @escaping (String) -> Void) {
        self.service = service
        self.done = done
        super.init()
    }

    private func finish(_ reply: String) {
        guard !finished else { return }
        finished = true
        service.stop()
        service.delegate = nil
        MacResolve.active[ObjectIdentifier(self)] = nil
        done(reply)
    }

    func netServiceDidResolveAddress(_ sender: NetService) {
        guard let host = sender.hostName, !host.isEmpty, sender.port > 0 else {
            finish(errorReply(code: ReplyCode.notSupported))
            return
        }
        finish(encodeDictJson(["kind": "resolved", "host": host, "port": sender.port]))
    }

    func netService(_ sender: NetService, didNotResolve errorDict: [String: NSNumber]) {
        finish(errorReply(code: ReplyCode.unavailable))
    }
}

/// The announcement in force, per type. Only touched on the NetService thread.
final class MacAdvert: NSObject, NetServiceDelegate {
    static var current: [String: MacAdvert] = [:]

    let service: NetService
    private var done: ((String) -> Void)?

    init(service: NetService, done: @escaping (String) -> Void) {
        self.service = service
        self.done = done
        super.init()
    }

    func withdraw() {
        service.stop()
        service.delegate = nil
    }

    private func answer(_ reply: String) {
        guard let done = done else { return }
        self.done = nil
        done(reply)
    }

    func netServiceDidPublish(_ sender: NetService) {
        answer(encodeDictJson(["kind": "advertised", "name": sender.name]))
    }

    func netService(_ sender: NetService, didNotPublish errorDict: [String: NSNumber]) {
        for (type, advert) in MacAdvert.current where advert === self {
            MacAdvert.current[type] = nil
        }
        withdraw()
        answer(errorReply(code: ReplyCode.unavailable))
    }
}

extension MacLocalNetwork {
    /// 1...48 characters of `[A-Za-z0-9._-]`, and not an IPv4 literal. Same alphabet as
    /// `checked_advertise` in families/local_network.rs: no name, space, `:` or `@` fits.
    static func isToken(_ value: String) -> Bool {
        guard !value.isEmpty, value.utf8.count <= 48 else { return false }
        let ok = value.unicodeScalars.allSatisfy {
            ($0.isASCII && CharacterSet.alphanumerics.contains($0)) || $0 == "." || $0 == "_" || $0 == "-"
        }
        if !ok { return false }
        let ipv4 = value.allSatisfy { $0.isASCII && ($0.isNumber || $0 == ".") }
            && value.filter { $0 == "." }.count == 3
        return !ipv4
    }

    /// Resolve a Bonjour service (name + type) to host and port, with a bounded wait.
    static func resolve(_ args: [String: Any]) -> String {
        guard let serviceType = args["service_type"] as? String,
              allowedTypes.contains(serviceType) else {
            return errorReply(code: ReplyCode.notSupported)
        }
        guard let name = args["name"] as? String,
              !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return errorReply(code: "missing_argument")
        }
        let requestedMs = (args["timeout_ms"] as? Int) ?? 5_000
        let seconds = Double(max(1_000, min(requestedMs, 15_000))) / 1000

        return blockingCall(timeout: seconds + 2) { done in
            NetServiceLoop.shared.perform {
                let service = NetService(domain: "local.", type: serviceType, name: name, port: 0)
                let resolve = MacResolve(service: service, done: done)
                MacResolve.active[ObjectIdentifier(resolve)] = resolve
                service.delegate = resolve
                service.resolve(withTimeout: seconds)
            }
        }
    }

    /// Announce this Atelier: `_atelier._tcp`, TXT `{id, proto, role}`, on the port of the
    /// server it already runs. No address, no person's name, no secret.
    static func advertise(_ args: [String: Any]) -> String {
        guard let serviceType = args["service_type"] as? String,
              advertisableTypes.contains(serviceType) else {
            return errorReply(code: "unsupported")
        }
        guard let port = args["port"] as? Int, (1...65535).contains(port),
              let id = args["id"] as? String, isToken(id),
              let proto = args["proto"] as? String, isToken(proto),
              let role = args["role"] as? String, isToken(role) else {
            return errorReply(code: ReplyCode.internalError)
        }
        var name = "Atelier-" + String(id.prefix(8))
        if let given = args["name"] as? String {
            guard isToken(given) else { return errorReply(code: ReplyCode.internalError) }
            name = given
        }
        let txt: [String: Data] = [
            "id": Data(id.utf8), "proto": Data(proto.utf8), "role": Data(role.utf8),
        ]
        let instance = name

        return blockingCall(timeout: 12) { done in
            NetServiceLoop.shared.perform {
                MacAdvert.current[serviceType]?.withdraw()
                let service = NetService(
                    domain: "local.", type: serviceType, name: instance, port: Int32(port))
                service.setTXTRecord(NetService.data(fromTXTRecord: txt))
                let advert = MacAdvert(service: service, done: done)
                MacAdvert.current[serviceType] = advert
                service.delegate = advert
                service.publish()
            }
        }
    }

    /// Withdraw the announcement. Idempotent.
    static func unadvertise(_ args: [String: Any]) -> String {
        guard let serviceType = args["service_type"] as? String,
              advertisableTypes.contains(serviceType) else {
            return errorReply(code: "unsupported")
        }
        return blockingCall(timeout: 5) { done in
            NetServiceLoop.shared.perform {
                MacAdvert.current[serviceType]?.withdraw()
                MacAdvert.current[serviceType] = nil
                done(encodeDictJson(["kind": "unadvertised"]))
            }
        }
    }
}
