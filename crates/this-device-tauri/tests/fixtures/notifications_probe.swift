import Foundation
print("bundleIdentifier=\(Bundle.main.bundleIdentifier ?? "nil") ext=\(Bundle.main.bundleURL.pathExtension) centreDisponible=\(MacNotifications.centreDisponible)")
// Des arguments VALIDES pour chaque entrée (`schedule` lit `in_ms`, borné à [1 000, 30 j]) : sans
// garde, chaque chemin doit atteindre le centre, pas s'arrêter avant sur une erreur d'arguments.
let args: [String: Any] = ["id": "x", "title": "t", "body": "b", "in_ms": 1000]
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
