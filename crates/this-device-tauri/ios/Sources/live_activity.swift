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
