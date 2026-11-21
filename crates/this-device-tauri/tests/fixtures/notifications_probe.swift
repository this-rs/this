import Foundation
print("bundleIdentifier=\(Bundle.main.bundleIdentifier ?? "nil") ext=\(Bundle.main.bundleURL.pathExtension) centreDisponible=\(MacNotifications.centreDisponible)")
let args: [String: Any] = ["id": "x", "title": "t", "body": "b", "delay_ms": 1000, "at": 4102444800000]
let resultats: [(String, String)] = [
  ("currentState", MacNotifications.currentState()),
  ("status", MacNotifications.status()),
  ("request", MacNotifications.request()),
  ("post", MacNotifications.post(args)),
  ("cancel", MacNotifications.cancel(["id": "x"])),
  ("schedule", MacNotifications.schedule(args)),
  ("pending", MacNotifications.pending()),
]
for (n, r) in resultats { print("ENTRY \(n) -> \(r)") }
print("ALIVE")
