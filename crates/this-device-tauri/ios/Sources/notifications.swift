// Notification permission, for `Capability::Notifications` (UNUserNotificationCenter).
//
// Status reads the authorization without prompting the person. Request asks it (alert, sound,
// badge), then answers the state the system now reports. No Info.plist key is needed: iOS asks
// at runtime. This file plans no notification: scheduling is out of scope here.
//
// Answers are `Reply` values (see Reply.swift). The completion handlers of UNUserNotificationCenter
// run on an arbitrary queue, so every answer is hopped onto the main queue before `resolve`.

import Foundation
import Tauri
import UserNotifications

enum NotificationStatus {
  /// The permission state in the contract's words (`PermissionState`).
  static func name(_ status: UNAuthorizationStatus) -> String {
    switch status {
    case .authorized, .provisional: return "granted"
    case .denied: return "denied"
    // Not asked yet. `.ephemeral` (App Clips, iOS 14+) and any future case fall here too: the
    // target is iOS 13, so `.ephemeral` cannot be named without an availability check.
    case .notDetermined: return "prompt"
    default: return "prompt"
    }
  }

  /// Reads the current settings and answers the state through `answer`, on the main queue.
  static func read(_ answer: @escaping (Reply) -> Void) {
    UNUserNotificationCenter.current().getNotificationSettings { settings in
      let reply = Reply.status(state: name(settings.authorizationStatus))
      DispatchQueue.main.async { answer(reply) }
    }
  }
}

extension ThisDevicePlugin {
  /// The notification permission, read without prompting the person.
  @objc public func notificationsStatus(_ invoke: Invoke) {
    NotificationStatus.read { invoke.resolve($0) }
  }

  /// Asks the person for the notification permission, then answers the state the system reports.
  /// A request the system refuses to answer is an internal error, not a denial.
  @objc public func notificationsRequest(_ invoke: Invoke) {
    UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) {
      _, error in
      if error != nil {
        DispatchQueue.main.async { invoke.resolve(Reply.error(code: "internal")) }
        return
      }
      NotificationStatus.read { invoke.resolve($0) }
    }
  }
}
