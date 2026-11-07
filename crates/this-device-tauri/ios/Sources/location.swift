// Location of the person, on demand (CoreLocation): the permission state, the request, and one
// position fix. Background location is not here: it needs UIBackgroundModes `location` and the
// matching entitlement in the app, and a session that outlives the call.
//
// Info.plist keys, or the system terminates the app on the first request:
//   - `location`        : NSLocationWhenInUseUsageDescription
//   - `location_always` : NSLocationAlwaysAndWhenInUseUsageDescription
//
// Privacy: a coordinate is personal data. Nothing in this file logs one, and a fix is sent back
// to the caller only.

import CoreLocation
import Foundation
import Tauri

struct LocationArgs: Decodable {
  let capability: String
}

/// How long the system may keep the person on the authorization prompt before the request
/// answers with the state it has. The prompt is modal: this only stops an unanswered call hanging.
let locationRequestTimeoutSeconds: Double = 60

/// How long one position fix may take before it is refused as a timeout.
let locationSampleTimeoutSeconds: Double = 10

/// The permission state of a location capability, in the contract's words (`PermissionState`).
enum LocationPermission {
  /// Whether `capability` is one of the two location capabilities this file bridges.
  static func handles(_ capability: String) -> Bool {
    capability == "location" || capability == "location_always"
  }

  /// The state of a location capability, read without prompting the person. A capability that
  /// is not a location one is `unsupported`.
  static func state(capability: String) -> String {
    guard handles(capability) else { return "unsupported" }
    return name(current(), capability: capability)
  }

  /// The system's authorization, read without prompting. A manager is created only to read it.
  static func current() -> CLAuthorizationStatus {
    if #available(iOS 14.0, *) {
      return CLLocationManager().authorizationStatus
    }
    return CLLocationManager.authorizationStatus()
  }

  /// `location` is granted when in use or always. `location_always` is granted only when always;
  /// in use alone is `prompt`, because asking again can upgrade it.
  static func name(_ status: CLAuthorizationStatus, capability: String) -> String {
    switch status {
    case .authorizedAlways:
      return "granted"
    case .authorizedWhenInUse:
      return capability == "location_always" ? "prompt" : "granted"
    case .denied, .restricted:
      return "denied"
    case .notDetermined:
      return "prompt"
    @unknown default:
      return "prompt"
    }
  }
}

/// A position fix, as the Rust side reads it (`families/location.rs`). Encoded by hand so the
/// keys and the integer `ts_ms` match the contract exactly.
struct LocationFix: Encodable {
  let latitude: Double
  let longitude: Double
  let accuracyM: Double
  let tsMs: Int64

  private enum Key: String, CodingKey {
    case kind, latitude, longitude
    case accuracyM = "accuracy_m"
    case tsMs = "ts_ms"
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    // Tagged like every other answer of the bridge (`Reply`): `kind` first.
    try c.encode("location", forKey: .kind)
    try c.encode(latitude, forKey: .latitude)
    try c.encode(longitude, forKey: .longitude)
    try c.encode(accuracyM, forKey: .accuracyM)
    try c.encode(tsMs, forKey: .tsMs)
  }
}

extension ThisDevicePlugin {
  /// Asks the person for a location capability, when the system has not decided yet. Answers
  /// with the state after the answer (or the current state when nothing is asked).
  @objc public func locationRequest(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(LocationArgs.self)
    guard LocationPermission.handles(args.capability) else {
      invoke.resolve(Reply.status(state: "unsupported"))
      return
    }
    LocationRequestSession.run(invoke, capability: args.capability)
  }

  /// One position fix: a single location, with the accuracy in meters and the time in Unix ms.
  @objc public func locationSample(_ invoke: Invoke) {
    LocationSampleSession.run(invoke)
  }
}

/// Asks the system for a location permission and answers once the person has answered.
final class LocationRequestSession: NSObject, CLLocationManagerDelegate {
  // One session per call: the delegate is weak, so the session keeps itself alive until it answers.
  private static var active: [ObjectIdentifier: LocationRequestSession] = [:]

  private let invoke: Invoke
  private let capability: String
  private let manager = CLLocationManager()
  private var startStatus: CLAuthorizationStatus = .notDetermined
  private var finished = false

  private init(_ invoke: Invoke, capability: String) {
    self.invoke = invoke
    self.capability = capability
    super.init()
  }

  /// Called on the main thread: CLLocationManager and its delegate callbacks live there.
  static func run(_ invoke: Invoke, capability: String) {
    DispatchQueue.main.async {
      let session = LocationRequestSession(invoke, capability: capability)
      session.start()
    }
  }

  private func start() {
    startStatus = LocationPermission.current()
    let needsAlways = capability == "location_always"

    // Prompt only when the system has not decided, or when `location_always` is asked for while
    // only "in use" is granted (the system then offers the upgrade). Otherwise the answer is
    // already known: a denied state never prompts again.
    let needsPrompt: Bool
    switch startStatus {
    case .notDetermined:
      needsPrompt = true
    case .authorizedWhenInUse:
      needsPrompt = needsAlways
    default:
      needsPrompt = false
    }
    guard needsPrompt else {
      finish(LocationPermission.name(startStatus, capability: capability))
      return
    }

    ThisDeviceLocationKeeper.keep(self)
    manager.delegate = self
    if needsAlways {
      manager.requestAlwaysAuthorization()
    } else {
      manager.requestWhenInUseAuthorization()
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + locationRequestTimeoutSeconds) { [weak self] in
      guard let self = self else { return }
      self.finish(LocationPermission.name(LocationPermission.current(), capability: self.capability))
    }
  }

  // iOS 14 and later. The protocol only declares this method from iOS 14, so the attribute
  // is not a restriction: on iOS 13 the delegate callback below is the one the system calls.
  @available(iOS 14.0, *)
  func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
    decided(manager.authorizationStatus)
  }

  // Before iOS 14.
  func locationManager(_ manager: CLLocationManager, didChangeAuthorization status: CLAuthorizationStatus) {
    decided(status)
  }

  /// The system reports the state at once when the delegate is set: a status equal to the one
  /// at start is not an answer. A change is.
  private func decided(_ status: CLAuthorizationStatus) {
    guard status != .notDetermined, status != startStatus else { return }
    finish(LocationPermission.name(status, capability: capability))
  }

  private func finish(_ state: String) {
    guard !finished else { return }
    finished = true
    manager.delegate = nil
    invoke.resolve(Reply.status(state: state))
    ThisDeviceLocationKeeper.release(self)
  }
}

/// Keeps a location session alive until it answers. Kept apart from the sessions themselves so
/// each one owns only its own state.
enum ThisDeviceLocationKeeper {
  private static var sessions: [ObjectIdentifier: AnyObject] = [:]

  static func keep(_ session: AnyObject) {
    sessions[ObjectIdentifier(session)] = session
  }

  static func release(_ session: AnyObject) {
    sessions[ObjectIdentifier(session)] = nil
  }
}

/// One position fix: a single `requestLocation()`, answered by the first usable location, or
/// refused after `locationSampleTimeoutSeconds`.
final class LocationSampleSession: NSObject, CLLocationManagerDelegate {
  private let invoke: Invoke
  private let manager = CLLocationManager()
  private var finished = false

  private init(_ invoke: Invoke) {
    self.invoke = invoke
    super.init()
  }

  /// Called on the main thread.
  static func run(_ invoke: Invoke) {
    DispatchQueue.main.async {
      guard CLLocationManager.locationServicesEnabled() else {
        // Location services are off for the whole device.
        invoke.resolve(Reply.error(code: ReplyCode.unavailable))
        return
      }
      switch LocationPermission.current() {
      case .authorizedAlways, .authorizedWhenInUse:
        let session = LocationSampleSession(invoke)
        ThisDeviceLocationKeeper.keep(session)
        session.start()
      default:
        // Denied, restricted, or never asked: a sample never shows the prompt. `locationRequest`
        // is the call that asks.
        invoke.resolve(Reply.error(code: ReplyCode.notAuthorized))
      }
    }
  }

  private func start() {
    manager.desiredAccuracy = kCLLocationAccuracyBest
    manager.delegate = self
    manager.requestLocation()
    DispatchQueue.main.asyncAfter(deadline: .now() + locationSampleTimeoutSeconds) { [weak self] in
      self?.finish(Reply.error(code: ReplyCode.timeout))
    }
  }

  func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
    // A negative horizontal accuracy marks an invalid fix: skip it, keep waiting.
    guard let location = locations.last(where: { $0.horizontalAccuracy >= 0 }),
      CLLocationCoordinate2DIsValid(location.coordinate)
    else { return }
    let tsMs = Int64((location.timestamp.timeIntervalSince1970 * 1000).rounded())
    finish(
      LocationFix(
        latitude: location.coordinate.latitude,
        longitude: location.coordinate.longitude,
        accuracyM: location.horizontalAccuracy,
        tsMs: tsMs))
  }

  func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
    if let clError = error as? CLError {
      switch clError.code {
      case .denied:
        finish(Reply.error(code: ReplyCode.notAuthorized))
        return
      case .locationUnknown:
        // Transient: the system keeps trying. The timeout answers if it does not get a fix.
        return
      default:
        break
      }
    }
    finish(Reply.error(code: ReplyCode.unavailable))
  }

  private func finish<T: Encodable>(_ reply: T) {
    guard !finished else { return }
    finished = true
    manager.delegate = nil
    manager.stopUpdatingLocation()
    invoke.resolve(reply)
    ThisDeviceLocationKeeper.release(self)
  }
}

// ---- Le flux de positions ---------------------------------------------------------------
//
// `locationSample` donne un point. Un point ne dit pas où l'on va, et le redemander en boucle
// depuis la page coûte un aller-retour par seconde. Ici le manager reste vivant entre deux
// appels, garde les positions dans un tampon borné, et l'appelant les relit.
//
// Une position est une donnée personnelle : rien de ce fichier ne l'écrit dans un journal.

private struct StreamStartArgs: Decodable {
  let accuracy: String
  let distanceFilterM: Double

  private enum CodingKeys: String, CodingKey {
    case accuracy
    case distanceFilterM = "distance_filter_m"
  }
}

private struct RecentArgs: Decodable {
  let sinceMs: Int64

  private enum CodingKeys: String, CodingKey {
    case sinceMs = "since_ms"
  }
}

/// Une position du tampon : latitude, longitude, précision en mètres, instant.
private struct StreamFixOut: Encodable {
  let lat: Double
  let lon: Double
  let accuracy_m: Double
  let ts_ms: Int64
}

private struct StreamingReply: Encodable {
  let kind = "streaming"
  let accuracy: String
  let distance_filter_m: Double
}

private struct StoppedReply: Encodable {
  let kind = "stopped"
  let fixes: Int
}

private struct FixesReply: Encodable {
  let kind = "fixes"
  let fixes: [StreamFixOut]
}

/// Le flux, retenu pour la vie de l'app : un `CLLocationManager` relâché cesse de livrer.
final class LocationStream: NSObject, CLLocationManagerDelegate {
  static let shared = LocationStream()
  /// Le plus de positions gardées : un flux sans borne est un historique qui grossit.
  static let ringMax = 200

  private let manager = CLLocationManager()
  private var fixes: [StreamFixOut] = []
  private(set) var running = false

  override init() {
    super.init()
    manager.delegate = self
    manager.desiredAccuracy = kCLLocationAccuracyHundredMeters
  }

  static func accuracy(_ name: String) -> CLLocationAccuracy? {
    switch name {
    case "three_kilometers": return kCLLocationAccuracyThreeKilometers
    case "hundred_meters": return kCLLocationAccuracyHundredMeters
    case "ten_meters": return kCLLocationAccuracyNearestTenMeters
    case "best": return kCLLocationAccuracyBest
    case "best_for_navigation": return kCLLocationAccuracyBestForNavigation
    default: return nil
    }
  }

  func start(name: String, distanceFilter: Double) -> Bool {
    guard let accuracy = Self.accuracy(name) else { return false }
    manager.desiredAccuracy = accuracy
    manager.distanceFilter = distanceFilter
    running = true
    manager.startUpdatingLocation()
    return true
  }

  func stop() -> Int {
    manager.stopUpdatingLocation()
    running = false
    return fixes.count
  }

  // Rend `StreamFixOut`, qui est privé : Swift exige que la visibilité suive. L'appel
  // (ligne 439) est dans ce même fichier.
  fileprivate func recent(sinceMs: Int64) -> [StreamFixOut] {
    fixes.filter { $0.ts_ms >= sinceMs }
  }

  func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
    for location in locations {
      fixes.append(
        StreamFixOut(
          lat: location.coordinate.latitude,
          lon: location.coordinate.longitude,
          accuracy_m: location.horizontalAccuracy,
          ts_ms: Int64((location.timestamp.timeIntervalSince1970 * 1000).rounded())
        ))
    }
    if fixes.count > Self.ringMax {
      fixes.removeFirst(fixes.count - Self.ringMax)
    }
  }

  /// L'autorisation en cours : un flux qui n'a pas le droit de lire ne doit pas être annoncé
  /// comme démarré.
  static func authorized() -> Bool {
    // `authorizationStatus` en propriété est iOS 14+ : le test de disponibilité doit donc
    // ENCADRER la lecture, pas la suivre. Avant 14, c'est la méthode de classe.
    // Les deux branches étaient identiques — la condition ne décidait de rien.
    if #available(iOS 14.0, *) {
      let status = CLLocationManager().authorizationStatus
      return status == .authorizedWhenInUse || status == .authorizedAlways
    }
    let status = CLLocationManager.authorizationStatus()
    return status == .authorizedWhenInUse || status == .authorizedAlways
  }
}

extension ThisDevicePlugin {
  /// Démarre le flux de positions. La permission doit déjà être accordée (`locationRequest`).
  @objc public func locationStreamStart(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(StreamStartArgs.self)
    guard LocationStream.authorized() else {
      invoke.resolve(Reply.error(code: ReplyCode.notAuthorized))
      return
    }
    guard LocationStream.shared.start(name: args.accuracy, distanceFilter: args.distanceFilterM)
    else {
      invoke.resolve(Reply.error(code: "internal"))
      return
    }
    invoke.resolve(
      StreamingReply(accuracy: args.accuracy, distance_filter_m: args.distanceFilterM))
  }

  @objc public func locationStreamStop(_ invoke: Invoke) {
    invoke.resolve(StoppedReply(fixes: LocationStream.shared.stop()))
  }

  /// Les positions reçues depuis `since_ms`. Vide veut dire « rien de neuf », pas une panne.
  @objc public func locationRecent(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(RecentArgs.self)
    invoke.resolve(FixesReply(fixes: LocationStream.shared.recent(sinceMs: args.sinceMs)))
  }
}
