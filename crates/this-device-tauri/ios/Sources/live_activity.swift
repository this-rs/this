// Live activity permission, for `Capability::LiveActivity` (ActivityKit).
//
// Status only: no activity is started here. Live activities have no permission dialog; the
// system reports whether they are enabled (the person's Settings, and the app's
// NSSupportsLiveActivities flag in Info.plist). Before iOS 16.1 there is no ActivityKit, and the
// state is `unsupported`.

import ActivityKit
import Foundation
import Tauri

extension ThisDevicePlugin {
  /// The live activity state, read without prompting the person.
  @objc public func liveActivityStatus(_ invoke: Invoke) {
    if #available(iOS 16.1, *) {
      // `false` covers both "the person turned them off" and "the app lacks the Info.plist
      // flag": ActivityKit does not tell the two apart, so both are `denied`.
      let enabled = ActivityAuthorizationInfo().areActivitiesEnabled
      invoke.resolve(Reply.status(state: enabled ? "granted" : "denied"))
    } else {
      invoke.resolve(Reply.status(state: "unsupported"))
    }
  }
}

// MARK: - Démarrer, mettre à jour, terminer
//
// DUPLICATION VOULUE — NE PAS FACTORISER. ActivityKit apparie l'app et l'extension de widget
// par le NOM et la FORME du type d'attributs, pas par un module partagé. Cette struct doit donc
// rester identique, nom et champs, à `AtelierAttributes` de
// `atelier/src-tauri/gen/apple/Widget/AtelierWidget.swift`. La « factoriser » dans un seul
// fichier casse l'appariement sans erreur de compilation : l'activité démarre, rien ne s'affiche.

#if canImport(ActivityKit)
@available(iOS 16.1, *)
struct AtelierAttributes: ActivityAttributes {
  struct ContentState: Codable, Hashable {
    var titre: String
    var detail: String
    var progression: Double?  // 0…1
  }
  var nom: String = "Atelier"
}
#endif

/// Arguments des trois méthodes (le JSON de `ActivityRequest::args()` côté Rust).
struct LiveActivityArgs: Decodable {
  let id: String?
  let titre: String?
  let detail: String?
  let progression: Double?
}

/// Réponse de la famille : `{"kind":"activity","id":…}` ou `{"kind":"error","code":…}`.
/// Encodée à la main, comme `Reply` : `id` absent doit devenir `null`.
private func activityReply(id: String?) -> [String: Any] {
  ["kind": "activity", "id": id as Any? ?? NSNull()]
}

private func activityError(_ code: String) -> [String: Any] {
  ["kind": "error", "code": code]
}

extension ThisDevicePlugin {
  /// Démarre une activité. Rend son identifiant, ou une erreur nommée — jamais un succès muet.
  @objc public func liveActivityStart(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(LiveActivityArgs.self)
    guard #available(iOS 16.1, *) else {
      invoke.resolve(activityError("unavailable")); return
    }
    guard let titre = args.titre, !titre.trimmingCharacters(in: .whitespaces).isEmpty else {
      invoke.resolve(activityError("titre_absent")); return
    }
    // Réglage coupé par la personne, ou NSSupportsLiveActivities absent : ActivityKit ne les
    // distingue pas. On refuse AVANT `request`, qui lèverait une erreur peu parlante.
    guard ActivityAuthorizationInfo().areActivitiesEnabled else {
      invoke.resolve(activityError("not_authorized")); return
    }
    let state = AtelierAttributes.ContentState(
      titre: titre, detail: args.detail ?? "", progression: args.progression)
    do {
      let activity = try Activity<AtelierAttributes>.request(
        attributes: AtelierAttributes(),
        contentState: state,
        pushType: nil)
      invoke.resolve(activityReply(id: activity.id))
    } catch {
      // Trop d'activités, app en arrière-plan, etc. : on trace la cause, on répond `unavailable`.
      NSLog("this-device liveActivityStart a échoué : \(error)")
      invoke.resolve(activityError("unavailable"))
    }
  }

  /// Met à jour l'activité `id`. Une activité inconnue (déjà terminée, ou d'un autre lancement)
  /// est `unknown_activity`, pas un succès.
  @objc public func liveActivityUpdate(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(LiveActivityArgs.self)
    guard #available(iOS 16.1, *) else {
      invoke.resolve(activityError("unavailable")); return
    }
    guard let id = args.id, !id.isEmpty else {
      invoke.resolve(activityError("id_absent")); return
    }
    guard let titre = args.titre, !titre.trimmingCharacters(in: .whitespaces).isEmpty else {
      invoke.resolve(activityError("titre_absent")); return
    }
    guard let activity = Activity<AtelierAttributes>.activities.first(where: { $0.id == id })
    else {
      invoke.resolve(activityError("unknown_activity")); return
    }
    let state = AtelierAttributes.ContentState(
      titre: titre, detail: args.detail ?? "", progression: args.progression)
    Task {
      await activity.update(using: state)
      invoke.resolve(activityReply(id: activity.id))
    }
  }

  /// Termine l'activité `id` et la retire tout de suite de l'écran verrouillé.
  @objc public func liveActivityEnd(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(LiveActivityArgs.self)
    guard #available(iOS 16.1, *) else {
      invoke.resolve(activityError("unavailable")); return
    }
    guard let id = args.id, !id.isEmpty else {
      invoke.resolve(activityError("id_absent")); return
    }
    guard let activity = Activity<AtelierAttributes>.activities.first(where: { $0.id == id })
    else {
      invoke.resolve(activityError("unknown_activity")); return
    }
    Task {
      await activity.end(using: nil, dismissalPolicy: .immediate)
      invoke.resolve(activityReply(id: nil))
    }
  }
}
