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
        case .authorized, .provisional: return "granted"
        case .denied: return "denied"
        case .notDetermined: return "prompt"
        @unknown default: return "prompt"
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
                guard state == "granted" else {
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
}
