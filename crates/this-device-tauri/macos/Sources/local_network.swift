// Local network discovery on macOS (Network framework, NWBrowser / Bonjour).
//
// macOS does NOT have the local network permission dialog that iOS 14+ shows. Any app
// can browse Bonjour services freely. So the status is always "granted".
//
// Browse: one NWBrowser on one service type from the closed list, for 5 seconds.
// The answer carries the names and types of the services found, nothing else: no endpoint
// is resolved, so no IP address leaves this file.

import Foundation
import Network

enum MacLocalNetwork {
    /// The only service types a browse may name. Mirrors the iOS side and families/local_network.rs.
    static let allowedTypes: [String] = ["_http._tcp"]
    /// How long the browse listens.
    static let browseSeconds: TimeInterval = 5

    // MARK: - Permission state

    /// macOS has no local network permission: always granted.
    static func currentState() -> String {
        return "granted"
    }

    // MARK: - Commands

    /// Status: always granted on macOS (no permission dialog).
    static func status() -> String {
        return statusReply(state: "granted")
    }

    /// One Bonjour browse of a type in the closed list, for a few seconds.
    /// Returns the services found (name, type, domain).
    static func browse(_ args: [String: Any]) -> String {
        let serviceType = (args["service_type"] as? String) ?? "_http._tcp"
        guard allowedTypes.contains(serviceType) else {
            return errorReply(code: "unsupported")
        }

        return blockingCall(timeout: browseSeconds + 2) { done in
            let browser = NWBrowser(
                for: .bonjour(type: serviceType, domain: nil),
                using: NWParameters()
            )

            var foundResults: Set<NWBrowser.Result> = []
            let lock = NSLock()

            browser.browseResultsChangedHandler = { results, _ in
                lock.lock()
                foundResults = results
                lock.unlock()
            }

            browser.stateUpdateHandler = { state in
                switch state {
                case .failed(let error):
                    browser.cancel()
                    done(errorReply(code: ReplyCode.unavailable))
                    _ = error  // Logged for debugging, not exposed.
                default:
                    break
                }
            }

            browser.start(queue: .global(qos: .userInitiated))

            // Wait the browse window, then collect results.
            DispatchQueue.global(qos: .userInitiated).asyncAfter(
                deadline: .now() + browseSeconds
            ) {
                browser.cancel()

                lock.lock()
                let snapshot = foundResults
                lock.unlock()

                let devices: [[String: Any]] = snapshot.compactMap { result in
                    guard case let .service(name, svcType, domain, _) = result.endpoint else {
                        return nil
                    }
                    return [
                        "name": name,
                        "type": svcType,
                        "domain": domain ?? "local."
                    ]
                }
                .sorted { ($0["name"] as? String ?? "") < ($1["name"] as? String ?? "") }

                done(encodeDictJson([
                    "kind": "local_network",
                    "devices": devices
                ]))
            }
        }
    }
}
