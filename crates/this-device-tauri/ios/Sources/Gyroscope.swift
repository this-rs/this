// One gyroscope sample, on demand.
//
// Raw gyroscope data (CMGyroData) needs no permission and no Info.plist key: unlike
// CMMotionActivity or the pedometer, which need NSMotionUsageDescription. The first sample
// that arrives is returned and the updates stop. Nothing within `timeoutSeconds` is an error.

import CoreMotion
import Foundation
import Tauri

final class GyroscopeSession {
  static let timeoutSeconds: Double = 2

  // One session per call: two concurrent calls never stop each other's updates.
  private static var active: [ObjectIdentifier: GyroscopeSession] = [:]

  private let motion = CMMotionManager()
  private let invoke: Invoke
  private var finished = false

  private init(_ invoke: Invoke) {
    self.invoke = invoke
  }

  /// Starts a session and keeps it alive until it answers. Called on the main thread.
  static func sample(_ invoke: Invoke) {
    guard CMMotionManager().isGyroAvailable else {
      // Simulator, or a device without a gyroscope.
      invoke.resolve(Reply.error(code: ReplyCode.unavailable))
      return
    }
    let session = GyroscopeSession(invoke)
    active[ObjectIdentifier(session)] = session
    session.start()
  }

  private func start() {
    motion.gyroUpdateInterval = 1.0 / 60.0
    motion.startGyroUpdates(to: .main) { [weak self] data, _ in
      guard let self = self, let data = data else { return }
      let sample = data.rotationRate
      // CMLogItem.timestamp counts seconds since boot. Converted to Unix epoch milliseconds
      // so the value can be compared with the clock of the app.
      let age = ProcessInfo.processInfo.systemUptime - data.timestamp
      let epochMs = (Date().timeIntervalSince1970 - age) * 1000
      self.finish(
        Reply.gyroscope(
          x: sample.x, y: sample.y, z: sample.z, tsMs: Int64(epochMs.rounded())))
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + GyroscopeSession.timeoutSeconds) {
      [weak self] in
      self?.finish(Reply.error(code: ReplyCode.timeout))
    }
  }

  private func finish(_ reply: Reply) {
    guard !finished else { return }
    finished = true
    motion.stopGyroUpdates()
    invoke.resolve(reply)
    GyroscopeSession.active[ObjectIdentifier(self)] = nil
  }
}
