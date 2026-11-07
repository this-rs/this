// Local network, for `Capability::LocalNetwork` (Network framework, Bonjour browse).
//
// Two commands, both answering through the contract's `Reply` (see Reply.swift), with the family
// answers `services` and the codes `unsupported` added for this family (see families/local_network.rs).
//
// Status. iOS has no API that reads the local network permission without asking for it: the
// only signal is an NWBrowser that actually runs, and running one can show the system prompt.
// So the status answers from what earlier browses observed, and never claims more:
//   - `granted`: a browse found at least one service (proof of a grant);
//   - `denied`: a browse was refused by the system (proof of a refusal);
//   - `prompt`: nothing observed yet in this install. Unknown, not a promise of a prompt.
// The observation is kept in UserDefaults. It goes stale if the person changes the setting in
// Réglages: a stale `granted` is corrected by the next browse, which then answers `not_authorized`.
//
// Browse. One NWBrowser on one service type of the closed list below, for `browseSeconds`. The
// answer carries the names and the types of the services found, nothing else: no endpoint is
// resolved, so no IP address leaves this file. Every type browsed must also be listed in
// NSBonjourServices of Info.plist, or iOS refuses the browse.

import Foundation
import Network
import Tauri

enum LocalNetworkServiceTypes {
  /// The only service types a browse may name. Mirrors `SERVICE_TYPES` in
  /// families/local_network.rs: change both together, and both must match NSBonjourServices.
  static let allowed: [String] = ["_http._tcp"]

  static func isAllowed(_ serviceType: String) -> Bool {
    allowed.contains(serviceType)
  }
}

/// The last outcome a browse observed, kept across launches. Read by the status command.
enum LocalNetworkRecord {
  private static let key = "this-device.local-network.last-outcome"

  static var last: String? {
    UserDefaults.standard.string(forKey: key)
  }

  static func remember(_ outcome: String) {
    UserDefaults.standard.set(outcome, forKey: key)
  }
}

/// A service found by a browse: its name and its type, nothing more.
struct FoundService {
  let name: String
  let serviceType: String
}

extension FoundService: Encodable {
  private enum Key: String, CodingKey {
    case name
    case serviceType = "service_type"
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    try c.encode(name, forKey: .name)
    try c.encode(serviceType, forKey: .serviceType)
  }
}

/// The answer of a successful browse: `{"kind":"services","services":[...]}`.
struct ServicesReply: Encodable {
  let services: [FoundService]

  private enum Key: String, CodingKey {
    case kind, services
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    try c.encode("services", forKey: .kind)
    try c.encode(services, forKey: .services)
  }
}

struct LocalNetworkBrowseArgs: Decodable {
  let serviceType: String

  private enum CodingKeys: String, CodingKey {
    case serviceType = "service_type"
  }
}

/// One browse of one service type. The session keeps itself alive until it answers, since the
/// caller only holds the `Invoke`. Runs on the main queue.
final class LocalNetworkBrowse {
  /// How long the browse listens. A sample of what is around, not a background listener.
  static let browseSeconds: Double = 5

  /// The `NWError.dns` code the system reports when the local network permission is refused
  /// (kDNSServiceErr_PolicyDenied). Not a documented constant: it is the value observed for
  /// this refusal, and it is not verified on a device here.
  static let policyDeniedCode: Int32 = -65570

  private static var active: [ObjectIdentifier: LocalNetworkBrowse] = [:]

  private let invoke: Invoke
  private let serviceType: String
  private var browser: NWBrowser?
  private var results: Set<NWBrowser.Result> = []
  private var finished = false

  private init(invoke: Invoke, serviceType: String) {
    self.invoke = invoke
    self.serviceType = serviceType
  }

  /// Starts a browse. Called on the main thread with a type the caller has already checked.
  static func run(_ invoke: Invoke, serviceType: String) {
    let session = LocalNetworkBrowse(invoke: invoke, serviceType: serviceType)
    active[ObjectIdentifier(session)] = session
    session.start()
  }

  private func start() {
    let browser = NWBrowser(
      for: .bonjour(type: serviceType, domain: nil), using: NWParameters())
    self.browser = browser

    browser.browseResultsChangedHandler = { [weak self] results, _ in
      self?.results = results
    }
    browser.stateUpdateHandler = { [weak self] state in
      switch state {
      case .waiting(let error), .failed(let error):
        // A refusal is final for this browse: answer now rather than after the window.
        self?.refused(error)
      default:
        break
      }
    }
    browser.start(queue: .main)

    DispatchQueue.main.asyncAfter(deadline: .now() + LocalNetworkBrowse.browseSeconds) {
      [weak self] in
      self?.answer()
    }
  }

  /// The window is over: answer the services seen, sorted so the answer is stable.
  private func answer() {
    let services = results
      .compactMap { result -> FoundService? in
        guard case let .service(name, serviceTypeName, _, _) = result.endpoint else { return nil }
        return FoundService(name: name, serviceType: serviceTypeName)
      }
      .sorted { ($0.name, $0.serviceType) < ($1.name, $1.serviceType) }
    // Services found are the proof that the permission is granted.
    if !services.isEmpty {
      LocalNetworkRecord.remember("granted")
    }
    finish(ServicesReply(services: services))
  }

  /// The browser reported an error. The policy code is the system's refusal; anything else is
  /// a radio or network that cannot answer now.
  private func refused(_ error: NWError) {
    if case let .dns(code) = error, code == LocalNetworkBrowse.policyDeniedCode {
      LocalNetworkRecord.remember("denied")
      finish(Reply.error(code: "not_authorized"))
    } else {
      finish(Reply.error(code: "unavailable"))
    }
  }

  private func finish<T: Encodable>(_ reply: T) {
    guard !finished else { return }
    finished = true
    browser?.cancel()
    browser = nil
    invoke.resolve(reply)
    LocalNetworkBrowse.active[ObjectIdentifier(self)] = nil
  }
}

// The commands. Registered by the plugin as `localNetworkStatus` and `localNetworkBrowse`:
// `@objc` methods of `ThisDevicePlugin`, dispatched by name from the Rust side.
extension ThisDevicePlugin {
  /// The local network state, as far as earlier browses observed it. Never prompts.
  @objc public func localNetworkStatus(_ invoke: Invoke) {
    let state: String
    switch LocalNetworkRecord.last {
    case "granted"?: state = "granted"
    case "denied"?: state = "denied"
    default: state = "prompt"
    }
    invoke.resolve(Reply.status(state: state))
  }

  /// One Bonjour browse of a type in the closed list, for a few seconds. Answers the services
  /// found (name and type), or an error.
  @objc public func localNetworkBrowse(_ invoke: Invoke) {
    guard let args = try? invoke.parseArgs(LocalNetworkBrowseArgs.self) else {
      invoke.resolve(Reply.error(code: "internal"))
      return
    }
    // The Rust side checks the list first. This is the same check, kept here so a native call
    // can never browse a type the list does not name.
    guard LocalNetworkServiceTypes.isAllowed(args.serviceType) else {
      invoke.resolve(Reply.error(code: "unsupported"))
      return
    }
    LocalNetworkBrowse.run(invoke, serviceType: args.serviceType)
  }
}

// ---- Résoudre un service ---------------------------------------------------------------
//
// Un browse rend des noms (« MacBook de Théotime ») ; pour s'y connecter il faut un hôte et un
// port. `NetService.resolve` les donne, avec une borne de temps — sans elle, un service qui ne
// répond pas laisserait la promesse ouverte pour toujours.
//
// L'hôte trouvé est une donnée d'infrastructure (un nom sur le réseau local), pas une donnée de
// la personne : il est rendu à l'appelant, jamais journalisé.

private struct ResolveArgs: Decodable {
  let name: String
  let serviceType: String
  let timeoutMs: Int

  private enum CodingKeys: String, CodingKey {
    case name
    case serviceType = "service_type"
    case timeoutMs = "timeout_ms"
  }
}

private struct ResolvedReply: Encodable {
  let kind = "resolved"
  let host: String
  let port: Int
}

/// Une résolution en vol. Retenue jusqu'à la réponse : `NetService` ne garde pas son délégué.
final class LocalNetworkResolve: NSObject, NetServiceDelegate {
  private static var active: [ObjectIdentifier: LocalNetworkResolve] = [:]

  private let invoke: Invoke
  private let service: NetService
  private var finished = false

  private init(_ invoke: Invoke, service: NetService) {
    self.invoke = invoke
    self.service = service
    super.init()
  }

  static func run(_ invoke: Invoke, name: String, serviceType: String, timeoutSeconds: Double) {
    let service = NetService(
      domain: "local.", type: serviceType, name: name, port: 0)
    let resolve = LocalNetworkResolve(invoke, service: service)
    active[ObjectIdentifier(resolve)] = resolve
    service.delegate = resolve
    service.resolve(withTimeout: timeoutSeconds)
  }

  private func finish(_ reply: Encodable) {
    guard !finished else { return }
    finished = true
    service.stop()
    service.delegate = nil
    LocalNetworkResolve.active[ObjectIdentifier(self)] = nil
    DispatchQueue.main.async { self.invoke.resolve(reply) }
  }

  func netServiceDidResolveAddress(_ sender: NetService) {
    // `hostName` peut être nil alors que l'adresse est résolue : on préfère un refus explicite à
    // une chaîne vide que l'appelant prendrait pour un hôte.
    guard let host = sender.hostName, !host.isEmpty, sender.port > 0 else {
      finish(Reply.error(code: ReplyCode.notSupported))
      return
    }
    finish(ResolvedReply(host: host, port: sender.port))
  }

  func netService(_ sender: NetService, didNotResolve errorDict: [String: NSNumber]) {
    // Le service a disparu, ou il ne répond pas : ce n'est pas une panne du pont.
    finish(Reply.error(code: ReplyCode.unavailable))
  }
}

extension ThisDevicePlugin {
  /// Résout un service Bonjour (nom + type) en hôte et port, avec une borne de temps.
  @objc public func localNetworkResolve(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ResolveArgs.self)
    guard LocalNetworkServiceTypes.isAllowed(args.serviceType) else {
      invoke.resolve(Reply.error(code: "not_supported"))
      return
    }
    guard !args.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
      invoke.resolve(Reply.error(code: "missing_argument"))
      return
    }
    LocalNetworkResolve.run(
      invoke,
      name: args.name,
      serviceType: args.serviceType,
      timeoutSeconds: Double(max(1_000, min(args.timeoutMs, 15_000))) / 1000
    )
  }
}
