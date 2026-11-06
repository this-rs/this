// Clipboard (text only) and share sheet, on iOS.
//
// Raw primitives, like the rest of the plugin: no consent is checked here. The access decision
// belongs to the device broker (`this_platform::device_bridge::gate`), see lib.rs.
//
// Privacy: the text of the pasteboard and the text to share are never logged. Replies carry
// no content except the text the caller asked to read.
//
// Wire: every answer is one JSON object with a `kind`, encoded by hand like Reply.swift.
// The answer types are `ClipboardShareReply` (below), not `Reply`: the device contract has
// no clipboard or share variant yet.

import Foundation
import Tauri
import UIKit

/// Arguments of `clipboard_write` and `share_text`.
struct TextArgs: Decodable {
  let text: String
}

/// An answer of the clipboard and share commands. Encoded to the wire shapes of
/// `families/clipboard_share.rs`.
enum ClipboardShareReply: Encodable {
  /// The text on the pasteboard, or `null` when it holds nothing.
  case clipboardText(String?)
  /// The text was written to the pasteboard.
  case clipboardWritten
  /// The share sheet closed. `completed` is true when the person chose a destination.
  case shared(completed: Bool)
  /// A refusal or failure, as a `DeviceError` code: `not_supported`, `unavailable`, `internal`.
  case error(code: String)

  private enum Key: String, CodingKey {
    case kind, text, completed, code
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    switch self {
    case let .clipboardText(text):
      try c.encode("clipboard_text", forKey: .kind)
      // `encode` (not `encodeIfPresent`) writes `null` for an empty pasteboard.
      try c.encode(text, forKey: .text)
    case .clipboardWritten:
      try c.encode("clipboard_written", forKey: .kind)
    case let .shared(completed):
      try c.encode("shared", forKey: .kind)
      try c.encode(completed, forKey: .completed)
    case let .error(code):
      try c.encode("error", forKey: .kind)
      try c.encode(code, forKey: .code)
    }
  }
}

extension ThisDevicePlugin {
  /// The text on the pasteboard. Text only: any other content is refused as `not_supported`.
  ///
  /// iOS 16 and later show the "pasted from" banner when the string is read. This method does
  /// not work around it: the banner is the person's information, and it stays.
  @objc public func clipboardRead(_ invoke: Invoke) {
    DispatchQueue.main.async {
      let pasteboard = UIPasteboard.general
      if pasteboard.hasStrings {
        // The read itself: this is the call that makes iOS show the banner.
        invoke.resolve(ClipboardShareReply.clipboardText(pasteboard.string))
      } else if pasteboard.numberOfItems == 0 {
        invoke.resolve(ClipboardShareReply.clipboardText(nil))
      } else {
        // An image, a URL, a file or another type: not text, so not read.
        invoke.resolve(ClipboardShareReply.error(code: "not_supported"))
      }
    }
  }

  /// Replaces the pasteboard content with `text`.
  @objc public func clipboardWrite(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(TextArgs.self)
    DispatchQueue.main.async {
      UIPasteboard.general.string = args.text
      invoke.resolve(ClipboardShareReply.clipboardWritten)
    }
  }

  /// Presents the system share sheet with `text` as the only item. Answers when the sheet
  /// closes.
  ///
  /// The presenter is `PluginManager.shared.viewController`, the controller that Tauri sets in
  /// `on_webview_created` (the one hosting the webview), walked to its topmost presented
  /// controller so the sheet is not presented over a sheet already on screen. Not verified on a
  /// device: when the controller is missing, the answer is `unavailable`.
  @objc public func shareText(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(TextArgs.self)
    DispatchQueue.main.async {
      guard let presenter = self.topmostPresenter() else {
        invoke.resolve(ClipboardShareReply.error(code: "unavailable"))
        return
      }
      let activity = UIActivityViewController(activityItems: [args.text], applicationActivities: nil)
      // iPad needs an anchor for the popover, or presentation fails. Tauri's helper anchors it
      // on the presenter's view.
      UIUtils.centerPopover(rootViewController: presenter, popoverController: activity)
      activity.completionWithItemsHandler = { _, completed, _, _ in
        invoke.resolve(ClipboardShareReply.shared(completed: completed))
      }
      presenter.present(activity, animated: true)
    }
  }

  /// The controller that presents the share sheet: the webview's controller, or the one it
  /// presents if any, down to the topmost. Nil before the webview exists.
  private func topmostPresenter() -> UIViewController? {
    guard var top = manager.viewController else { return nil }
    while let presented = top.presentedViewController {
      top = presented
    }
    return top
  }
}
