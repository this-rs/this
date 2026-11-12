// Activity and steps, on demand: CMMotionActivityManager (walking, running, cycling, driving) and
// CMPedometer (step counts). Both sit under one permission, Motion & Fitness, declared by
// NSMotionUsageDescription in Info.plist. Without that key the system ends the app on first use.
//
// This is not the raw gyroscope (Gyroscope.swift): nothing here reads sensors, only what iOS
// has computed from them.

import CoreMotion
import Foundation
import Tauri

struct MotionStepsArgs: Decodable {
  let fromMs: Int64
  let toMs: Int64

  private enum CodingKeys: String, CodingKey {
    case fromMs = "from_ms"
    case toMs = "to_ms"
  }
}

/// The answer of the steps command. Kept out of Reply.swift: a step count is not a shape of the
/// device contract (`this_platform::device_bridge::Reply`). The status and error answers of this
/// family reuse `Reply`, so the Rust side reads them with the same codes as the other commands.
enum MotionStepsReply: Encodable {
  case steps(count: Int, fromMs: Int64, toMs: Int64)

  private enum Key: String, CodingKey {
    case kind, steps, from_ms, to_ms
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    switch self {
    case let .steps(count, fromMs, toMs):
      try c.encode("steps", forKey: .kind)
      try c.encode(count, forKey: .steps)
      try c.encode(fromMs, forKey: .from_ms)
      try c.encode(toMs, forKey: .to_ms)
    }
  }
}

/// The permission state of Motion & Fitness, in the contract's words (`PermissionState`).
enum MotionAuthorization {
  static func name(_ status: CMAuthorizationStatus) -> String {
    switch status {
    case .authorized: return "granted"
    case .denied, .restricted: return "denied"
    // Not asked yet: the system asks the first time the motion data is read.
    case .notDetermined: return "prompt"
    @unknown default: return "prompt"
    }
  }
}

extension ThisDevicePlugin {
  /// The permission state of activity, read without prompting the person. `unsupported` when the
  /// hardware has no activity manager (simulator, some iPads).
  @objc public func motionActivityStatus(_ invoke: Invoke) {
    guard CMMotionActivityManager.isActivityAvailable() else {
      invoke.resolve(Reply.status(state: "unsupported"))
      return
    }
    invoke.resolve(
      Reply.status(state: MotionAuthorization.name(CMMotionActivityManager.authorizationStatus())))
  }

  /// Asks for the Motion & Fitness permission, then answers with the state that results.
  @objc public func motionActivityRequest(_ invoke: Invoke) {
    MotionActivityRequest.run(invoke)
  }

  /// The steps counted between `from_ms` and `to_ms` (Unix epoch milliseconds), as the pedometer
  /// answers for that window. The Rust side bounds the window; see families/motion_activity.rs.
  @objc public func motionStepsSample(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(MotionStepsArgs.self)
    MotionStepsSample.run(invoke, fromMs: args.fromMs, toMs: args.toMs)
  }
}

/// The permission request of activity. Apple gives no request API: the prompt appears on the first
/// read of motion data, so this reads a short window of history and answers with the state after
/// it. If the read does not return within `timeoutSeconds`, the state at that moment is answered.
final class MotionActivityRequest {
  static let timeoutSeconds: Double = 5

  // One session per call; kept alive until it answers, since the caller only holds the `Invoke`.
  private static var active: [ObjectIdentifier: MotionActivityRequest] = [:]

  private let manager = CMMotionActivityManager()
  private let invoke: Invoke
  private var finished = false

  private init(_ invoke: Invoke) {
    self.invoke = invoke
  }

  /// Called on the main thread.
  static func run(_ invoke: Invoke) {
    guard CMMotionActivityManager.isActivityAvailable() else {
      invoke.resolve(Reply.status(state: "unsupported"))
      return
    }
    let request = MotionActivityRequest(invoke)
    active[ObjectIdentifier(request)] = request
    request.start()
  }

  private func start() {
    let now = Date()
    // The window is one minute of history. Its content is not used: the read is the request.
    manager.queryActivityStarting(from: now.addingTimeInterval(-60), to: now, to: .main) {
      [weak self] _, _ in
      self?.finish()
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + MotionActivityRequest.timeoutSeconds) {
      [weak self] in
      self?.finish()
    }
  }

  private func finish() {
    guard !finished else { return }
    finished = true
    invoke.resolve(
      Reply.status(state: MotionAuthorization.name(CMMotionActivityManager.authorizationStatus())))
    MotionActivityRequest.active[ObjectIdentifier(self)] = nil
  }
}

/// One step count over a window, from the pedometer. Read on demand, never as a stream.
final class MotionStepsSample {
  static let timeoutSeconds: Double = 10

  // One session per call; kept alive until it answers.
  private static var active: [ObjectIdentifier: MotionStepsSample] = [:]

  private let pedometer = CMPedometer()
  private let invoke: Invoke
  private var finished = false

  private init(_ invoke: Invoke) {
    self.invoke = invoke
  }

  /// Called on the main thread with a window the Rust side has already validated.
  static func run(_ invoke: Invoke, fromMs: Int64, toMs: Int64) {
    guard CMPedometer.isStepCountingAvailable() else {
      invoke.resolve(Reply.error(code: "not_supported"))
      return
    }
    // Read without prompting: a denied or restricted permission cannot be read anyway.
    let auth = CMPedometer.authorizationStatus()
    if auth == .denied || auth == .restricted {
      invoke.resolve(Reply.error(code: "not_authorized"))
      return
    }
    let session = MotionStepsSample(invoke)
    active[ObjectIdentifier(session)] = session
    session.start(
      from: Date(timeIntervalSince1970: Double(fromMs) / 1000),
      to: Date(timeIntervalSince1970: Double(toMs) / 1000))
  }

  private func start(from: Date, to: Date) {
    pedometer.queryPedometerData(from: from, to: to) { [weak self] data, _ in
      // The handler runs on a queue of the system: the answer is taken back on the main thread.
      DispatchQueue.main.async {
        guard let self = self else { return }
        if let data = data {
          self.finish(
            MotionStepsReply.steps(
              count: data.numberOfSteps.intValue,
              fromMs: MotionStepsSample.epochMs(data.startDate),
              toMs: MotionStepsSample.epochMs(data.endDate)))
          return
        }
        // No data: either the person refused in the meantime, or the read failed.
        let auth = CMPedometer.authorizationStatus()
        let refused = auth == .denied || auth == .restricted
        self.finish(Reply.error(code: refused ? "not_authorized" : "unavailable"))
      }
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + MotionStepsSample.timeoutSeconds) {
      [weak self] in
      self?.finish(Reply.error(code: "timeout"))
    }
  }

  private func finish<T: Encodable>(_ reply: T) {
    guard !finished else { return }
    finished = true
    invoke.resolve(reply)
    MotionStepsSample.active[ObjectIdentifier(self)] = nil
  }

  private static func epochMs(_ date: Date) -> Int64 {
    Int64((date.timeIntervalSince1970 * 1000).rounded())
  }
}

// ---- Distance et étages ---------------------------------------------------------------
//
// Le podomètre donne la distance et les étages en même temps que les pas. « Combien de
// kilomètres » est la question qu'on pose réellement ; la réponse tient dans la même requête.

private struct MotionDistanceArgs: Decodable {
  let fromMs: Int64
  let toMs: Int64

  private enum CodingKeys: String, CodingKey {
    case fromMs = "from_ms"
    case toMs = "to_ms"
  }
}

private struct MotionDistanceReply: Encodable {
  let kind = "distance"
  let meters: Double
  let floors_up: Int
  let floors_down: Int
}

extension ThisDevicePlugin {
  /// La distance et les étages d'une fenêtre. Les deux sont facultatifs côté système : un
  /// appareil sans baromètre rend `floors_up` à 0, ce qui est une réponse, pas une panne.
  @objc public func motionDistance(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(MotionDistanceArgs.self)
    guard CMPedometer.isDistanceAvailable() || CMPedometer.isFloorCountingAvailable() else {
      invoke.resolve(Reply.error(code: ReplyCode.notSupported))
      return
    }
    guard args.toMs > args.fromMs else {
      invoke.resolve(Reply.error(code: "internal"))
      return
    }
    let pedometer = CMPedometer()
    // Le podomètre n'a pas de poignée à garder : la fermeture retient l'objet pendant l'appel.
    MotionDistanceSession.active.insert(ObjectIdentifier(pedometer))
    pedometer.queryPedometerData(
      from: Date(timeIntervalSince1970: Double(args.fromMs) / 1000),
      to: Date(timeIntervalSince1970: Double(args.toMs) / 1000)
    ) { data, error in
      MotionDistanceSession.active.remove(ObjectIdentifier(pedometer))
      guard error == nil, let data = data else {
        DispatchQueue.main.async { invoke.resolve(Reply.error(code: "internal")) }
        return
      }
      let reply = MotionDistanceReply(
        meters: data.distance?.doubleValue ?? 0,
        floors_up: data.floorsAscended?.intValue ?? 0,
        floors_down: data.floorsDescended?.intValue ?? 0
      )
      DispatchQueue.main.async { invoke.resolve(reply) }
    }
  }
}

/// Garde les podomètres en vol : sans ça, l'objet peut être libéré avant la réponse et la
/// promesse d'invocation ne serait jamais résolue.
enum MotionDistanceSession {
  static var active: Set<ObjectIdentifier> = []
}

/// Ancre de l'éditeur de liens — voir `ThisDevicePlugin.famillesAncrees`.
///
/// Un `#selector(Foo.bar)` ne produit pas une référence au SYMBOLE de `bar` : il produit une
/// chaîne. Le dead-strip retire donc l'objet entier, et le pont répond « No command bar found
/// for plugin this-device ». Mesuré le 13/11/2026 sur un iPhone 16 Pro : dix-huit méthodes de
/// familles compilaient, étaient listées dans `famillesLiees`, et étaient **absentes du binaire**.
/// Référencer une CLASSE de ce fichier garde son objet — donc ses méthodes.
final class FamilleMotionActivity: NSObject {}
