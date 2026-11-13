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
  static let allowed: [String] = ["_http._tcp", "_atelier._tcp"]

  /// The only types an announcement may use: the Atelier's own, never `_http._tcp` (that would
  /// pose as a web server). Mirrors `ADVERTISE_TYPES` in families/local_network.rs.
  static let advertisable: [String] = ["_atelier._tcp"]

  static func isAdvertisable(_ serviceType: String) -> Bool {
    advertisable.contains(serviceType)
  }

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
      LocalNetworkRecord.remember(Etat.granted.fil)
    }
    finish(ServicesReply(services: services))
  }

  /// The browser reported an error. The policy code is the system's refusal; anything else is
  /// a radio or network that cannot answer now.
  private func refused(_ error: NWError) {
    if case let .dns(code) = error, code == LocalNetworkBrowse.policyDeniedCode {
      LocalNetworkRecord.remember(Etat.denied.fil)
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
    case Etat.granted.fil?: state = Etat.granted.fil
    case Etat.denied.fil?: state = Etat.denied.fil
    default: state = Etat.prompt.fil
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

// ---- Publier le service ----------------------------------------------------------------
//
// Un Atelier s'annonce sous `_atelier._tcp` avec un TXT de trois champs : `id`, `proto`, `role`.
// Jamais d'adresse (le système publie l'hôte lui-même), jamais de nom de personne, jamais de
// secret : les champs sont validés ici sur le même alphabet fermé que côté Rust
// (`checked_advertise` dans families/local_network.rs), donc un appel natif direct ne peut pas
// y glisser autre chose. Le port est celui du serveur que l'Atelier fait déjà tourner : on
// annonce, on n'écoute pas. Une seule annonce à la fois par type ; une nouvelle remplace l'ancienne.

private struct AdvertiseArgs: Decodable {
  let serviceType: String
  let port: Int
  let id: String
  let proto: String
  let role: String
  let name: String?

  private enum CodingKeys: String, CodingKey {
    case serviceType = "service_type"
    case port, id, proto, role, name
  }
}

private struct UnadvertiseArgs: Decodable {
  let serviceType: String

  private enum CodingKeys: String, CodingKey {
    case serviceType = "service_type"
  }
}

private struct AdvertisedReply: Encodable {
  let kind = "advertised"
  let name: String
}

private struct UnadvertisedReply: Encodable {
  let kind = "unadvertised"
}

enum LocalNetworkTxt {
  static let maxLen = 48

  /// 1...48 caractères de `[A-Za-z0-9._-]` : rien de libre n'y tient (ni nom, ni espace, ni `:`).
  static func isToken(_ value: String) -> Bool {
    guard !value.isEmpty, value.utf8.count <= maxLen else { return false }
    let ok = value.unicodeScalars.allSatisfy {
      ($0.isASCII && CharacterSet.alphanumerics.contains($0)) || $0 == "." || $0 == "_" || $0 == "-"
    }
    if !ok { return false }
    // Un littéral IPv4 (chiffres et trois points) n'est jamais un champ valide.
    let ipv4 = value.allSatisfy { $0.isASCII && ($0.isNumber || $0 == ".") }
      && value.filter { $0 == "." }.count == 3
    return !ipv4
  }
}

/// L'annonce en cours. `NetService` ne se retient pas lui-même : on la garde ici.
final class LocalNetworkAdvert: NSObject, NetServiceDelegate {
  private static var current: [String: LocalNetworkAdvert] = [:]

  private let service: NetService
  private var invoke: Invoke?

  private init(service: NetService, invoke: Invoke) {
    self.service = service
    self.invoke = invoke
    super.init()
  }

  static func run(
    _ invoke: Invoke, serviceType: String, name: String, port: Int, txt: [String: Data]
  ) {
    current[serviceType]?.withdraw()
    let service = NetService(domain: "local.", type: serviceType, name: name, port: Int32(port))
    service.setTXTRecord(NetService.data(fromTXTRecord: txt))
    let advert = LocalNetworkAdvert(service: service, invoke: invoke)
    current[serviceType] = advert
    service.delegate = advert
    service.publish()
  }

  static func stop(serviceType: String) {
    current[serviceType]?.withdraw()
    current[serviceType] = nil
  }

  private func withdraw() {
    service.stop()
    service.delegate = nil
  }

  private func answer(_ reply: Encodable) {
    guard let invoke = invoke else { return }
    self.invoke = nil
    DispatchQueue.main.async { invoke.resolve(reply) }
  }

  func netServiceDidPublish(_ sender: NetService) {
    answer(AdvertisedReply(name: sender.name))
  }

  func netService(_ sender: NetService, didNotPublish errorDict: [String: NSNumber]) {
    // NSNetServicesError : -72008 (NoAuth) quand le type n'est pas dans NSBonjourServices ou que
    // la permission réseau local est refusée ; le reste est un réseau qui ne répond pas.
    let code = errorDict[NetService.errorCode]?.intValue ?? 0
    for (type, advert) in LocalNetworkAdvert.current where advert === self {
      LocalNetworkAdvert.current[type] = nil
    }
    withdraw()
    answer(Reply.error(code: code == -72008 ? "not_authorized" : "unavailable"))
  }
}

extension ThisDevicePlugin {
  /// Annonce cet Atelier sur le réseau local : `_atelier._tcp`, TXT `{id, proto, role}`.
  /// Répond `advertised` (avec le nom publié) ou une erreur.
  @objc public func localNetworkAdvertise(_ invoke: Invoke) {
    guard let args = try? invoke.parseArgs(AdvertiseArgs.self) else {
      invoke.resolve(Reply.error(code: "internal"))
      return
    }
    guard LocalNetworkServiceTypes.isAdvertisable(args.serviceType) else {
      invoke.resolve(Reply.error(code: "unsupported"))
      return
    }
    guard (1...65535).contains(args.port),
      [args.id, args.proto, args.role].allSatisfy(LocalNetworkTxt.isToken),
      args.name.map(LocalNetworkTxt.isToken) ?? true
    else {
      invoke.resolve(Reply.error(code: "internal"))
      return
    }
    let name = args.name ?? "Atelier-" + String(args.id.prefix(8))
    LocalNetworkAdvert.run(
      invoke,
      serviceType: args.serviceType,
      name: name,
      port: args.port,
      txt: [
        "id": Data(args.id.utf8),
        "proto": Data(args.proto.utf8),
        "role": Data(args.role.utf8),
      ])
  }

  /// Retire l'annonce. Idempotent : sans annonce en cours, répond quand même `unadvertised`.
  @objc public func localNetworkUnadvertise(_ invoke: Invoke) {
    guard let args = try? invoke.parseArgs(UnadvertiseArgs.self) else {
      invoke.resolve(Reply.error(code: "internal"))
      return
    }
    guard LocalNetworkServiceTypes.isAdvertisable(args.serviceType) else {
      invoke.resolve(Reply.error(code: "unsupported"))
      return
    }
    LocalNetworkAdvert.stop(serviceType: args.serviceType)
    invoke.resolve(UnadvertisedReply())
  }
}

/// Ancre de l'éditeur de liens — voir `ThisDevicePlugin.famillesAncrees`.
///
/// Un `#selector(Foo.bar)` ne produit pas une référence au SYMBOLE de `bar` : il produit une
/// chaîne. Le dead-strip retire donc l'objet entier, et le pont répond « No command bar found
/// for plugin this-device ». Mesuré le 13/11/2026 sur un iPhone 16 Pro : dix-huit méthodes de
/// familles compilaient, étaient listées dans `famillesLiees`, et étaient **absentes du binaire**.
/// Référencer une CLASSE de ce fichier garde son objet — donc ses méthodes.
final class FamilleLocalNetwork: NSObject {}
