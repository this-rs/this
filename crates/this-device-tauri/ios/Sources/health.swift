// HealthKit, read only: the health family of the device bridge.
//
// Three commands, each answering with a `Reply` (Reply.swift) or, for samples, a
// `HealthSamplesReply` (`kind: "samples"`, the shape of `HealthReply` in src/families/health.rs):
//
// - healthStatus(quantity): the read state of one listed type, without prompting.
// - healthRequest(quantities): the permission sheet for listed types. Read only: nothing is
//   written, `toShare` is always empty.
// - healthQuantitySample(quantity, window_ms, limit): the most recent samples of one listed
//   type, newest first, within a bounded window.
//
// What iOS does not reveal: whether a READ was granted. HealthKit gives no read status, and an
// empty answer means "no data" or "no access", indistinguishable. So a status is `prompt` in
// every case, never `granted`, and an empty samples list is not a refusal.
//
// Prerequisites of the app bundle (not added here): the HealthKit entitlement
// (com.apple.developer.healthkit) and NSHealthShareUsageDescription in Info.plist. Without the
// entitlement, HealthKit calls fail; without the key, the system ends the app on the first read.
// No value read from HealthKit is ever logged.

import Foundation
import HealthKit
import Tauri

/// The closed list of quantity types the bridge reads. Mirrors `QuantityType` in
/// src/families/health.rs: a name outside it is refused.
enum HealthQuantity: String, CaseIterable {
  case stepCount = "step_count"
  case heartRate = "heart_rate"
  case bodyMass = "body_mass"

  var identifier: HKQuantityTypeIdentifier {
    switch self {
    case .stepCount: return .stepCount
    case .heartRate: return .heartRate
    case .bodyMass: return .bodyMass
    }
  }

  /// The HealthKit type. The identifiers above always exist, so the force unwrap cannot fail.
  var type: HKQuantityType {
    HKObjectType.quantityType(forIdentifier: identifier)!
  }

  /// The HealthKit unit each value is converted to before it is sent.
  var hkUnit: HKUnit {
    switch self {
    case .stepCount: return .count()
    case .heartRate: return HKUnit.count().unitDivided(by: .minute())
    case .bodyMass: return .gramUnit(with: .kilo)
    }
  }

  /// The unit name the answer carries: the same string as `QuantityType::unit` in Rust.
  var unitName: String {
    switch self {
    case .stepCount: return "count"
    case .heartRate: return "count/min"
    case .bodyMass: return "kg"
    }
  }
}

/// The bounds applied to a sample request, the same as in src/families/health.rs.
enum HealthBounds {
  static let windowMsMin = 60_000
  static let windowMsMax = 86_400_000
  static let limitMin = 1
  static let limitMax = 10
}

/// One HealthKit store for the process. Creating it is cheap; it is not shared with any other
/// module.
enum HealthKitSession {
  static let store = HKHealthStore()
}

struct HealthStatusArgs: Decodable {
  let quantity: String
}

struct HealthRequestArgs: Decodable {
  let quantities: [String]
}

struct HealthSampleArgs: Decodable {
  let quantity: String
  let windowMs: Int
  let limit: Int

  private enum CodingKeys: String, CodingKey {
    case quantity
    case windowMs = "window_ms"
    case limit
  }
}

/// One sample: its value in the type's unit, and its start and end in Unix epoch milliseconds.
struct HealthSample: Encodable {
  let value: Double
  let startMs: Int64
  let endMs: Int64

  private enum CodingKeys: String, CodingKey {
    case value
    case startMs = "start_ms"
    case endMs = "end_ms"
  }
}

/// The samples answer: `kind: "samples"`, with the quantity name, its unit and the samples.
struct HealthSamplesReply: Encodable {
  let quantity: String
  let unit: String
  let samples: [HealthSample]

  private enum Key: String, CodingKey {
    case kind, quantity, unit, samples
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    try c.encode("samples", forKey: .kind)
    try c.encode(quantity, forKey: .quantity)
    try c.encode(unit, forKey: .unit)
    try c.encode(samples, forKey: .samples)
  }
}

extension ThisDevicePlugin {
  /// The read state of one listed type, without prompting. Read only: HealthKit does not reveal
  /// a read permission, so the state is `prompt` whatever `authorizationStatus` reports for it.
  @objc public func healthStatus(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(HealthStatusArgs.self)
    guard HKHealthStore.isHealthDataAvailable() else {
      invoke.resolve(Reply.status(state: Etat.unsupported.fil))
      return
    }
    guard let quantity = HealthQuantity(rawValue: args.quantity) else {
      invoke.resolve(Reply.error(code: "not_supported"))
      return
    }
    // The write (share) status of the type is read for the record only: for a read type it
    // says nothing about what the person allowed, so it does not change the answer.
    _ = HealthKitSession.store.authorizationStatus(for: quantity.type)
    invoke.resolve(Reply.status(state: Etat.prompt.fil))
  }

  /// Asks read access for the listed types, and shows the permission sheet when iOS has not
  /// asked yet. Read only: `toShare` is empty.
  @objc public func healthRequest(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(HealthRequestArgs.self)
    guard HKHealthStore.isHealthDataAvailable() else {
      invoke.resolve(Reply.error(code: "unavailable"))
      return
    }
    let quantities = args.quantities.compactMap { HealthQuantity(rawValue: $0) }
    guard !quantities.isEmpty else {
      invoke.resolve(Reply.error(code: "internal"))
      return
    }
    guard quantities.count == args.quantities.count else {
      // One name outside the closed list refuses the whole request.
      invoke.resolve(Reply.error(code: "not_supported"))
      return
    }
    let readTypes = Set(quantities.map { $0.type as HKObjectType })
    HealthKitSession.store.requestAuthorization(toShare: [], read: readTypes) { _, error in
      // The completion runs on an arbitrary queue: the answer goes out on the main thread.
      DispatchQueue.main.async {
        if error != nil {
          invoke.resolve(Reply.error(code: "unavailable"))
          return
        }
        // The sheet was answered, or had been: iOS does not say which, and never says `granted`
        // for a read. An empty samples answer is how a missing permission shows up.
        invoke.resolve(Reply.status(state: Etat.prompt.fil))
      }
    }
  }

  /// The most recent samples of one listed type, newest first, within a bounded window.
  /// An empty list means no data or no read access: HealthKit does not tell which.
  @objc public func healthQuantitySample(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(HealthSampleArgs.self)
    guard HKHealthStore.isHealthDataAvailable() else {
      invoke.resolve(Reply.error(code: "unavailable"))
      return
    }
    guard let quantity = HealthQuantity(rawValue: args.quantity) else {
      invoke.resolve(Reply.error(code: "not_supported"))
      return
    }
    let windowMs = min(max(args.windowMs, HealthBounds.windowMsMin), HealthBounds.windowMsMax)
    let limit = min(max(args.limit, HealthBounds.limitMin), HealthBounds.limitMax)
    let now = Date()
    let start = now.addingTimeInterval(-Double(windowMs) / 1000)
    let predicate = HKQuery.predicateForSamples(withStart: start, end: now, options: [])
    let newestFirst = NSSortDescriptor(key: HKSampleSortIdentifierEndDate, ascending: false)
    let query = HKSampleQuery(
      sampleType: quantity.type,
      predicate: predicate,
      limit: limit,
      sortDescriptors: [newestFirst]
    ) { _, results, error in
      DispatchQueue.main.async {
        if error != nil {
          invoke.resolve(Reply.error(code: "unavailable"))
          return
        }
        let found = (results as? [HKQuantitySample]) ?? []
        let samples = found.map { sample in
          HealthSample(
            value: sample.quantity.doubleValue(for: quantity.hkUnit),
            startMs: Int64((sample.startDate.timeIntervalSince1970 * 1000).rounded()),
            endMs: Int64((sample.endDate.timeIntervalSince1970 * 1000).rounded())
          )
        }
        invoke.resolve(
          HealthSamplesReply(
            quantity: quantity.rawValue, unit: quantity.unitName, samples: samples))
      }
    }
    HealthKitSession.store.execute(query)
  }
}

// ---- Les séances -----------------------------------------------------------------------
//
// Une quantité dit « 8 000 pas » ; elle ne dit pas « j'ai couru 40 minutes hier ». `HKWorkout`
// porte l'activité et ses bornes. Pas de calories ni de distance : elles varient par type et par
// montre, et une valeur absente serait lue comme un zéro.
//
// HealthKit ne révèle PAS une autorisation de lecture : une liste vide peut vouloir dire « pas de
// séance » ou « pas d'accès ». C'est une réponse, pas une erreur — et le module pur le dit.

private struct WorkoutsArgs: Decodable {
  let fromMs: Int64
  let toMs: Int64
  let limit: Int

  private enum CodingKeys: String, CodingKey {
    case fromMs = "from_ms"
    case toMs = "to_ms"
    case limit
  }
}

private struct WorkoutOut: Encodable {
  let activity: String
  let startMs: Int64
  let endMs: Int64
  let durationS: Int64

  private enum Key: String, CodingKey {
    case activity, duration_s
    case start_ms
    case end_ms
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    try c.encode(activity, forKey: .activity)
    try c.encode(startMs, forKey: .start_ms)
    try c.encode(endMs, forKey: .end_ms)
    try c.encode(durationS, forKey: .duration_s)
  }
}

private struct WorkoutsReply: Encodable {
  let kind = "workouts"
  let workouts: [WorkoutOut]
}

extension ThisDevicePlugin {
  /// L'activité d'une séance, en mots stables. Un type non listé est `other` : inventer un nom
  /// pour chaque variante d'Apple donnerait des libellés qui changent d'une version à l'autre.
  private func workoutName(_ type: HKWorkoutActivityType) -> String {
    switch type {
    case .running: return "running"
    case .walking: return "walking"
    case .cycling: return "cycling"
    case .swimming: return "swimming"
    case .hiking: return "hiking"
    case .yoga: return "yoga"
    case .functionalStrengthTraining, .traditionalStrengthTraining: return "strength"
    case .rowing: return "rowing"
    case .elliptical: return "elliptical"
    case .highIntensityIntervalTraining: return "hiit"
    default: return "other"
    }
  }

  /// Les séances d'une fenêtre, les plus récentes d'abord.
  @objc public func healthWorkouts(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(WorkoutsArgs.self)
    guard HKHealthStore.isHealthDataAvailable() else {
      invoke.resolve(Reply.error(code: "unsupported"))
      return
    }
    guard args.toMs > args.fromMs, args.limit > 0, args.limit <= 50 else {
      invoke.resolve(Reply.error(code: "internal"))
      return
    }
    let start = Date(timeIntervalSince1970: Double(args.fromMs) / 1000)
    let end = Date(timeIntervalSince1970: Double(args.toMs) / 1000)
    let predicate = HKQuery.predicateForSamples(withStart: start, end: end, options: [])
    let newestFirst = NSSortDescriptor(key: HKSampleSortIdentifierEndDate, ascending: false)
    let query = HKSampleQuery(
      sampleType: HKObjectType.workoutType(),
      predicate: predicate,
      limit: args.limit,
      sortDescriptors: [newestFirst]
    ) { _, results, error in
      DispatchQueue.main.async {
        if error != nil {
          invoke.resolve(Reply.error(code: "unavailable"))
          return
        }
        let found = (results as? [HKWorkout]) ?? []
        let workouts = found.map { workout in
          WorkoutOut(
            activity: self.workoutName(workout.workoutActivityType),
            startMs: Int64((workout.startDate.timeIntervalSince1970 * 1000).rounded()),
            endMs: Int64((workout.endDate.timeIntervalSince1970 * 1000).rounded()),
            durationS: Int64(workout.duration.rounded())
          )
        }
        invoke.resolve(WorkoutsReply(workouts: workouts))
      }
    }
    HealthKitSession.store.execute(query)
  }
}

/// Ancre de l'éditeur de liens — voir `ThisDevicePlugin.famillesAncrees`.
///
/// Un `#selector(Foo.bar)` ne produit pas une référence au SYMBOLE de `bar` : il produit une
/// chaîne. Le dead-strip retire donc l'objet entier, et le pont répond « No command bar found
/// for plugin this-device ». Mesuré le 13/11/2026 sur un iPhone 16 Pro : dix-huit méthodes de
/// familles compilaient, étaient listées dans `famillesLiees`, et étaient **absentes du binaire**.
/// Référencer une CLASSE de ce fichier garde son objet — donc ses méthodes.
final class FamilleHealth: NSObject {}
