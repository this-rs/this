// Notification permission, for `Capability::Notifications` (UNUserNotificationCenter).
//
// Status reads the authorization without prompting the person. Request asks it (alert, sound,
// badge), then answers the state the system now reports. No Info.plist key is needed: iOS asks
// at runtime. Post shows a notification at once (no schedule, no trigger); Cancel withdraws one.
//
// Answers are `Reply` values (see Reply.swift). The completion handlers of UNUserNotificationCenter
// run on an arbitrary queue. The STATUS READ answers from that queue directly (see
// `NotificationStatus.read`); the other methods still hop onto the main queue before `resolve`.

import Foundation
import Tauri
import UserNotifications

enum NotificationStatus {
  /// The permission state in the contract's words (`PermissionState`).
  static func name(_ status: UNAuthorizationStatus) -> String {
    switch status {
    case .authorized, .provisional: return Etat.granted.fil
    case .denied: return Etat.denied.fil
    // Not asked yet. `.ephemeral` (App Clips, iOS 14+) and any future case fall here too: the
    // target is iOS 13, so `.ephemeral` cannot be named without an availability check.
    case .notDetermined: return Etat.prompt.fil
    default: return Etat.prompt.fil
    }
  }

  /// Reads the current settings and answers the state through `answer`, from the queue the system
  /// calls back on.
  ///
  /// No hop onto the main queue, on purpose: a status read is passive and `Invoke.resolve` is safe
  /// from any queue (contacts and the other families resolve straight from their completion
  /// handlers). A `DispatchQueue.main.async` here made the answer depend on the main queue being
  /// free; measured on a device, a direct `notificationsStatus` call then never answered while the
  /// same read through the autotest did. Every other status method (`captureStatus`,
  /// `contactsStatus`...) already answers without the main queue; this one now does too.
  static func read(_ answer: @escaping (Reply) -> Void) {
    UNUserNotificationCenter.current().getNotificationSettings { settings in
      answer(Reply.status(state: name(settings.authorizationStatus)))
    }
  }
}

/// Arguments of `notificationsPost`.
struct NotificationPostArgs: Decodable {
  let id: String
  let title: String
  let body: String
}

/// Arguments of `notificationsCancel`.
struct NotificationCancelArgs: Decodable {
  let id: String
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

  /// Shows a local notification now. `id` names it: posting again with the same `id` replaces the
  /// one on screen instead of stacking a second.
  ///
  /// A notification is not a way around consent: when the system authorization is not granted,
  /// nothing is posted and the answer is `not_authorized`. The permission is read, never asked
  /// here — asking is `notificationsRequest`, a separate, visible act.
  /// Title and body are never logged.
  @objc public func notificationsPost(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(NotificationPostArgs.self)
    let center = UNUserNotificationCenter.current()
    center.getNotificationSettings { settings in
      let state = NotificationStatus.name(settings.authorizationStatus)
      guard state == Etat.granted.fil else {
        DispatchQueue.main.async { invoke.resolve(Reply.error(code: ReplyCode.notAuthorized)) }
        return
      }
      let content = UNMutableNotificationContent()
      content.title = args.title
      content.body = args.body
      content.sound = .default
      // `trigger: nil` = delivered immediately; the same identifier replaces a delivered one.
      let request = UNNotificationRequest(identifier: args.id, content: content, trigger: nil)
      center.add(request) { error in
        let reply = error == nil ? Reply.status(state: state) : Reply.error(code: "internal")
        DispatchQueue.main.async { invoke.resolve(reply) }
      }
    }
  }

  /// Withdraws the notification `id`, shown or still pending. Withdrawing one that is not there
  /// is not an error: the goal (it is gone) is met. Needs no authorization.
  @objc public func notificationsCancel(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(NotificationCancelArgs.self)
    let center = UNUserNotificationCenter.current()
    center.removeDeliveredNotifications(withIdentifiers: [args.id])
    center.removePendingNotificationRequests(withIdentifiers: [args.id])
    NotificationStatus.read { invoke.resolve($0) }
  }
}

// ---- Planifier, et savoir ce qui attend -----------------------------------------------
//
// `notificationsPost` ne sait que poster tout de suite : rappeler quelque chose dans une heure
// obligeait l'app à rester ouverte. Ces deux méthodes utilisent le déclencheur de temps du
// système, et rendent la liste de ce qui attend — sans quoi une notification planifiée est
// invisible jusqu'à ce qu'elle sonne.

private struct NotificationScheduleArgs: Decodable {
  let title: String
  let body: String
  let id: String
  let inMs: Int64

  private enum CodingKeys: String, CodingKey {
    case title, body, id
    case inMs = "in_ms"
  }
}

/// Les bornes d'une planification, alignées sur `families/notif_live.rs`.
enum ScheduleLimits {
  static let delayMsMin: Int64 = 1_000
  static let delayMsMax: Int64 = 30 * 24 * 60 * 60 * 1000
  static let pendingMax = 50
}

private struct NotificationScheduledReply: Encodable {
  let kind = "scheduled"
  let id: String
  let in_ms: Int64
}

private struct PendingEntryOut: Encodable {
  let id: String
  let title: String
  let in_ms: Int64
}

private struct NotificationsPendingReply: Encodable {
  let kind = "pending"
  let notifications: [PendingEntryOut]
}

extension ThisDevicePlugin {
  /// Planifie une notification dans `in_ms` millisecondes. Un identifiant qui existe déjà
  /// remplace la planification précédente, comme le fait le système.
  @objc public func notificationsSchedule(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(NotificationScheduleArgs.self)
    guard args.inMs >= ScheduleLimits.delayMsMin, args.inMs <= ScheduleLimits.delayMsMax,
      !args.title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    else {
      invoke.resolve(Reply.error(code: "internal"))
      return
    }
    let center = UNUserNotificationCenter.current()
    center.getNotificationSettings { settings in
      let state = NotificationStatus.name(settings.authorizationStatus)
      guard state == Etat.granted.fil else {
        DispatchQueue.main.async { invoke.resolve(Reply.error(code: ReplyCode.notAuthorized)) }
        return
      }
      let content = UNMutableNotificationContent()
      content.title = args.title
      content.body = args.body
      content.sound = .default
      let trigger = UNTimeIntervalNotificationTrigger(
        timeInterval: Double(args.inMs) / 1000, repeats: false)
      let request = UNNotificationRequest(identifier: args.id, content: content, trigger: trigger)
      center.add(request) { error in
        let reply: Encodable =
          error == nil
          ? NotificationScheduledReply(id: args.id, in_ms: args.inMs)
          : Reply.error(code: "internal")
        DispatchQueue.main.async { invoke.resolve(reply) }
      }
    }
  }

  /// Ce qui attend de sonner, dans l'ordre où le système le rendra. Ne demande aucune
  /// autorisation : c'est une lecture de ce que l'app a elle-même planifié.
  @objc public func notificationsPending(_ invoke: Invoke) {
    UNUserNotificationCenter.current().getPendingNotificationRequests { requests in
      let entries = requests.prefix(ScheduleLimits.pendingMax).map { request -> PendingEntryOut in
        // Le délai restant vient du déclencheur : c'est lui qui dit « dans combien de temps »,
        // et il n'y a pas d'autre source pour une planification encore en attente.
        var inMs: Int64 = 0
        if let trigger = request.trigger as? UNTimeIntervalNotificationTrigger {
          inMs = Int64(trigger.timeInterval * 1000)
        }
        return PendingEntryOut(
          id: request.identifier,
          title: request.content.title,
          in_ms: inMs
        )
      }
      DispatchQueue.main.async {
        invoke.resolve(NotificationsPendingReply(notifications: Array(entries)))
      }
    }
  }
}

/// Ancre de l'éditeur de liens — voir `ThisDevicePlugin.famillesAncrees`.
///
/// Un `#selector(Foo.bar)` ne produit pas une référence au SYMBOLE de `bar` : il produit une
/// chaîne. Le dead-strip retire donc l'objet entier, et le pont répond « No command bar found
/// for plugin this-device ». Mesuré le 13/11/2026 sur un iPhone 16 Pro : dix-huit méthodes de
/// familles compilaient, étaient listées dans `famillesLiees`, et étaient **absentes du binaire**.
/// Référencer une CLASSE de ce fichier garde son objet — donc ses méthodes.
final class FamilleNotifications: NSObject {}
