// Clipboard and share sheet, on iOS.
//
// Raw primitives, like the rest of the plugin: no consent is checked here. The access decision
// belongs to the device broker (`this_platform::device_bridge::gate`), see lib.rs.
//
// Privacy: what the person copied and what is shared are NEVER logged. A reply carries content
// only to the caller that asked for it.
//
// Wire: one JSON object with a `kind`, encoded by hand like Reply.swift (an `Optional` must
// become `null`, not an absent key). The shapes are `families/clipboard_share.rs`.
//
// ## What changed with the audit of 08/11/2026
//
// The pasteboard read only text and refused everything else as `not_supported`; the share sheet
// only took a String. A screenshot or a PDF could not be copied out of this app at all. Reading
// and writing now carry text, a link, an image; the share sheet takes those and a file.

import Foundation
import Tauri
import UIKit

/// Arguments of `shareText`.
struct TextArgs: Decodable {
  let text: String
}

/// Arguments of `clipboardWrite` and `shareItem`: exactly one payload, checked by the Rust side
/// (`families/clipboard_share.rs::checked_payload`). This side decodes what is present.
struct PayloadArgs: Decodable {
  let text: String?
  let url: String?
  let image_base64: String?
  let path: String?
}

/// An answer of the clipboard and share commands.
enum ClipboardShareReply: Encodable {
  /// The pasteboard, as whatever it holds. Every field is `null` when empty.
  case clipboard(text: String?, url: String?, imageBase64: String?, bytes: Int)
  /// The payload was written to the pasteboard.
  case clipboardWritten
  /// The share sheet closed. `completed` is true when the person chose a destination.
  case shared(completed: Bool)
  /// A refusal or failure, as the family's code: `not_supported`, `missing_argument`,
  /// `unavailable`, `too_large`, `bad_base64`, `internal`.
  case error(code: String)

  private enum Key: String, CodingKey {
    case kind, text, url, image_base64, bytes, completed, code
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    switch self {
    case let .clipboard(text, url, imageBase64, bytes):
      try c.encode("clipboard", forKey: .kind)
      // `encode` (not `encodeIfPresent`) writes `null` for what is not there: the Rust side
      // reads four fields, always.
      try c.encode(text, forKey: .text)
      try c.encode(url, forKey: .url)
      try c.encode(imageBase64, forKey: .image_base64)
      try c.encode(bytes, forKey: .bytes)
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
  /// The pasteboard, as whatever it holds: text, link, image. All three are read, because a
  /// screenshot copied from Photos is the common case and refusing it was the defect.
  ///
  /// iOS 16 and later show the "pasted from" banner when the content is read. This method does
  /// not work around it: the banner is the person's information, and it stays.
  @objc public func clipboardRead(_ invoke: Invoke) {
    DispatchQueue.main.async {
      let pasteboard = UIPasteboard.general
      if pasteboard.numberOfItems == 0 {
        invoke.resolve(
          ClipboardShareReply.clipboard(text: nil, url: nil, imageBase64: nil, bytes: 0))
        return
      }
      var text: String?
      var url: String?
      var image: String?
      var bytes = 0
      if pasteboard.hasStrings { text = pasteboard.string }
      if pasteboard.hasURLs { url = pasteboard.url?.absoluteString }
      if pasteboard.hasImages, let data = pasteboard.image?.pngData() {
        image = data.base64EncodedString()
        bytes = data.count
      }
      guard text != nil || url != nil || image != nil else {
        // Something this side cannot read (a file reference, an attributed string, a colour…):
        // refused, not guessed. The caller can still ask for the text explicitly.
        invoke.resolve(ClipboardShareReply.error(code: "not_supported"))
        return
      }
      invoke.resolve(
        ClipboardShareReply.clipboard(text: text, url: url, imageBase64: image, bytes: bytes))
    }
  }

  /// Replaces the pasteboard content with the one payload given.
  @objc public func clipboardWrite(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(PayloadArgs.self)
    DispatchQueue.main.async {
      let pasteboard = UIPasteboard.general
      // The order matters only for which branch wins; the Rust side already refused two
      // payloads at once, so exactly one of these is non-nil.
      if let text = args.text {
        pasteboard.string = text
      } else if let url = args.url, let parsed = URL(string: url) {
        pasteboard.url = parsed
      } else if let encoded = args.image_base64, let data = Data(base64Encoded: encoded),
        let image = UIImage(data: data)
      {
        pasteboard.image = image
      } else {
        invoke.resolve(ClipboardShareReply.error(code: "not_supported"))
        return
      }
      invoke.resolve(ClipboardShareReply.clipboardWritten)
    }
  }

  /// Presents the system share sheet with `text` as the only item. Answers when the sheet
  /// closes.
  ///
  /// The presenter is the webview's controller, walked to its topmost presented controller so
  /// the sheet is not presented over a sheet already on screen. When the controller is missing,
  /// the answer is `unavailable`.
  @objc public func shareText(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(TextArgs.self)
    DispatchQueue.main.async {
      self.presentShare(items: [args.text], invoke)
    }
  }

  /// Presents the system share sheet with the one payload given: a text, a link, an image, or
  /// a file of the app's container. Answers when the sheet closes.
  @objc public func shareItem(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(PayloadArgs.self)
    DispatchQueue.main.async {
      let items: [Any]
      if let text = args.text {
        items = [text]
      } else if let url = args.url, let parsed = URL(string: url) {
        items = [parsed]
      } else if let encoded = args.image_base64, let data = Data(base64Encoded: encoded),
        let image = UIImage(data: data)
      {
        items = [image]
      } else if let path = args.path {
        // A path the app cannot reach is not shareable: refusing is better than presenting an
        // empty sheet the person would read as a bug of the app.
        let url = URL(fileURLWithPath: path)
        guard FileManager.default.fileExists(atPath: url.path) else {
          invoke.resolve(ClipboardShareReply.error(code: "not_supported"))
          return
        }
        items = [url]
      } else {
        invoke.resolve(ClipboardShareReply.error(code: "missing_argument"))
        return
      }
      self.presentShare(items: items, invoke)
    }
  }

  /// Presents the sheet, anchored for iPad, and answers once it closes.
  private func presentShare(items: [Any], _ invoke: Invoke) {
    guard let presenter = topmostPresenter() else {
      invoke.resolve(ClipboardShareReply.error(code: "unavailable"))
      return
    }
    let activity = UIActivityViewController(activityItems: items, applicationActivities: nil)
    // iPad needs an anchor for the popover, or presentation fails. Tauri's helper anchors it
    // on the presenter's view.
    UIUtils.centerPopover(rootViewController: presenter, popoverController: activity)
    activity.completionWithItemsHandler = { _, completed, _, _ in
      invoke.resolve(ClipboardShareReply.shared(completed: completed))
    }
    presenter.present(activity, animated: true)
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

/// Ancre de l'éditeur de liens — voir `ThisDevicePlugin.famillesAncrees`.
///
/// Un `#selector(Foo.bar)` ne produit pas une référence au SYMBOLE de `bar` : il produit une
/// chaîne. Le dead-strip retire donc l'objet entier, et le pont répond « No command bar found
/// for plugin this-device ». Mesuré le 13/11/2026 sur un iPhone 16 Pro : dix-huit méthodes de
/// familles compilaient, étaient listées dans `famillesLiees`, et étaient **absentes du binaire**.
/// Référencer une CLASSE de ce fichier garde son objet — donc ses méthodes.
final class FamilleClipboardShare: NSObject {}
