// Notification permission, post and cancel on macOS (UserNotifications framework).
//
// Identical to iOS: UNUserNotificationCenter is the same API on macOS 10.14+.
// No Info.plist key is needed: macOS asks at runtime.
//
// Status reads the authorization without prompting. Request asks it (alert, sound, badge).
// Post shows a local notification at once; Cancel withdraws one by id.

import Foundation
import UserNotifications

enum MacNotifications {
    // MARK: - Permission state

    /// The permission state in the contract's words.
    static func stateName(_ status: UNAuthorizationStatus) -> String {
        switch status {
        case .authorized, .provisional: return Etat.granted.fil
        case .denied: return Etat.denied.fil
        case .notDetermined: return Etat.prompt.fil
        @unknown default: return Etat.prompt.fil
        }
    }

    /// The current permission state, for `macos_device_status`.
    static func currentState() -> String {
        return blockingCall { done in
            UNUserNotificationCenter.current().getNotificationSettings { settings in
                done(stateName(settings.authorizationStatus))
            }
        }
    }

    // MARK: - Commands

    /// The notification permission, read without prompting.
    static func status() -> String {
        return blockingCall { done in
            UNUserNotificationCenter.current().getNotificationSettings { settings in
                done(statusReply(state: stateName(settings.authorizationStatus)))
            }
        }
    }

    /// Asks the person for the notification permission, then answers the resulting state.
    static func request() -> String {
        return blockingCall(timeout: 120) { done in
            UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { _, error in
                if error != nil {
                    done(errorReply(code: ReplyCode.internalError))
                    return
                }
                UNUserNotificationCenter.current().getNotificationSettings { settings in
                    done(statusReply(state: stateName(settings.authorizationStatus)))
                }
            }
        }
    }

    /// Shows a local notification now. Same `id` replaces the previous one.
    /// Not authorized → `not_authorized`. Title and body are never logged.
    static func post(_ args: [String: Any]) -> String {
        guard let id = args["id"] as? String,
              let title = args["title"] as? String,
              let body = args["body"] as? String else {
            return errorReply(code: ReplyCode.internalError)
        }

        return blockingCall(timeout: 10) { done in
            let center = UNUserNotificationCenter.current()
            center.getNotificationSettings { settings in
                let state = stateName(settings.authorizationStatus)
                guard state == Etat.granted.fil else {
                    done(errorReply(code: ReplyCode.notAuthorized))
                    return
                }
                let content = UNMutableNotificationContent()
                content.title = title
                content.body = body
                content.sound = .default
                // `trigger: nil` = delivered immediately; same id replaces a delivered one.
                let request = UNNotificationRequest(identifier: id, content: content, trigger: nil)
                center.add(request) { error in
                    if error != nil {
                        done(errorReply(code: ReplyCode.internalError))
                    } else {
                        done(encodeDictJson(["kind": "notification_posted", "id": id]))
                    }
                }
            }
        }
    }

    /// Withdraws the notification `id`, shown or still pending. Withdrawing one that is not
    /// there is not an error: the goal (it is gone) is met.
    static func cancel(_ args: [String: Any]) -> String {
        guard let id = args["id"] as? String else {
            return errorReply(code: ReplyCode.internalError)
        }
        let center = UNUserNotificationCenter.current()
        center.removeDeliveredNotifications(withIdentifiers: [id])
        center.removePendingNotificationRequests(withIdentifiers: [id])
        return encodeDictJson(["kind": "notification_cancelled", "id": id])
    }

    // MARK: - Planifier, et savoir ce qui attend
    //
    // Il manquait les deux : poster tout de suite (ci-dessus) et retirer ne permettent pas de
    // rappeler quelque chose dans une heure sans laisser l'app ouverte. UserNotifications donne
    // le déclencheur de temps et la liste des attentes — les mêmes API que sur iOS, mesurées
    // identiques le 13/11/2026. Les formes de réponse sont celles que `families/notif_live.rs`
    // décode : `{"kind":"scheduled","id":…,"in_ms":…}` et `{"kind":"pending","notifications":[…]}`.

    /// Les bornes, alignées sur `notif_live.rs` (et donc sur iOS) : 1 s à 30 jours, 50 attentes.
    private enum ScheduleLimits {
        static let delayMsMin: Int64 = 1_000
        static let delayMsMax: Int64 = 30 * 24 * 60 * 60 * 1000
        static let pendingMax = 50
    }

    /// Un entier d'arguments JSON, quelle que soit la largeur avec laquelle il est arrivé.
    private static func entier(_ args: [String: Any], _ cle: String) -> Int64? {
        if let valeur = args[cle] as? Int64 { return valeur }
        if let valeur = args[cle] as? Int { return Int64(valeur) }
        if let valeur = args[cle] as? Double { return Int64(valeur) }
        return nil
    }

    /// Planifie une notification dans `in_ms` millisecondes. Un identifiant déjà présent
    /// remplace la planification précédente, comme le fait le système.
    static func schedule(_ args: [String: Any]) -> String {
        guard let id = args["id"] as? String,
              let title = args["title"] as? String,
              let body = args["body"] as? String,
              let inMs = entier(args, "in_ms"),
              inMs >= ScheduleLimits.delayMsMin,
              inMs <= ScheduleLimits.delayMsMax,
              !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else {
            return errorReply(code: ReplyCode.internalError)
        }

        return blockingCall(timeout: 10) { done in
            let center = UNUserNotificationCenter.current()
            center.getNotificationSettings { settings in
                let state = stateName(settings.authorizationStatus)
                guard state == Etat.granted.fil else {
                    done(errorReply(code: ReplyCode.notAuthorized))
                    return
                }
                let content = UNMutableNotificationContent()
                content.title = title
                content.body = body
                content.sound = .default
                let trigger = UNTimeIntervalNotificationTrigger(
                    timeInterval: Double(inMs) / 1000, repeats: false)
                let request = UNNotificationRequest(identifier: id, content: content, trigger: trigger)
                center.add(request) { error in
                    if error != nil {
                        done(errorReply(code: ReplyCode.internalError))
                    } else {
                        done(encodeDictJson(["kind": "scheduled", "id": id, "in_ms": inMs]))
                    }
                }
            }
        }
    }

    /// Ce qui attend, borné : `{"kind":"pending","notifications":[{"id","title","in_ms"}]}`.
    ///
    /// Le délai restant vient du déclencheur — c'est lui qui dit « dans combien de temps », et
    /// il n'y a pas d'autre source pour une planification encore en attente.
    static func pending() -> String {
        return blockingCall { done in
            UNUserNotificationCenter.current().getPendingNotificationRequests { requests in
                let entries: [[String: Any]] = requests.prefix(ScheduleLimits.pendingMax).map { request in
                    var inMs: Int64 = 0
                    if let trigger = request.trigger as? UNTimeIntervalNotificationTrigger {
                        inMs = Int64(trigger.timeInterval * 1000)
                    }
                    return [
                        "id": request.identifier,
                        "title": request.content.title,
                        "in_ms": inMs,
                    ]
                }
                done(encodeDictJson(["kind": "pending", "notifications": entries]))
            }
        }
    }
}
