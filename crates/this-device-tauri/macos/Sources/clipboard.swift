// Clipboard (text, link, image) on macOS, using NSPasteboard.
//
// Raw primitives: no consent is checked here. The access decision belongs to the device
// broker (`this_platform::device_bridge::gate`), see lib.rs.
//
// Privacy: the text of the pasteboard is never logged. No paste banner on macOS (unlike
// iOS 16+).
//
// Wire: matches the iOS clipboard_share.swift shapes exactly.

import AppKit
import Foundation

enum MacClipboard {
    /// Largest image `clipboardWrite` accepts, in decoded bytes (same bound as the Rust side).
    private static let imageMaxBytes = 4 * 1024 * 1024

    /// Read the general pasteboard as whatever it holds: text, link, image.
    /// Returns `{"kind":"clipboard","text":..|null,"url":..|null,"image_base64":..|null,"bytes":N}`.
    /// Something this side cannot read (files, colours...) is `not_supported`, not guessed.
    static func read() -> String {
        let pasteboard = NSPasteboard.general

        if pasteboard.pasteboardItems?.isEmpty ?? true {
            return clipboardReply(text: nil, url: nil, imagePng: nil)
        }

        let text = pasteboard.string(forType: .string)

        var url: String? = nil
        if let urls = pasteboard.readObjects(
            forClasses: [NSURL.self], options: nil) as? [URL],
           let first = urls.first, !first.isFileURL {
            url = first.absoluteString
        }

        var png: Data? = nil
        if let images = pasteboard.readObjects(forClasses: [NSImage.self], options: nil) as? [NSImage],
           let image = images.first {
            png = pngData(from: image)
        }

        guard text != nil || url != nil || png != nil else {
            return errorReply(code: ReplyCode.notSupported)
        }
        return clipboardReply(text: text, url: url, imagePng: png)
    }

    /// Replace the pasteboard content with the one payload given: `text`, `url` or `image_base64`.
    /// Returns `{"kind":"clipboard_written"}`.
    static func write(_ args: [String: Any]) -> String {
        let pasteboard = NSPasteboard.general
        let written = "{\"kind\":\"clipboard_written\"}"

        if let text = args["text"] as? String {
            pasteboard.clearContents()
            pasteboard.setString(text, forType: .string)
            return written
        }

        if let raw = args["url"] as? String {
            // Only a parseable link with a scheme, never a file reference.
            guard let url = URL(string: raw), url.scheme != nil, !url.isFileURL else {
                return errorReply(code: ReplyCode.notSupported)
            }
            pasteboard.clearContents()
            pasteboard.writeObjects([url as NSURL])
            return written
        }

        if let encoded = args["image_base64"] as? String {
            guard let data = Data(base64Encoded: encoded) else {
                return errorReply(code: "bad_base64")
            }
            guard data.count <= imageMaxBytes else {
                return errorReply(code: "too_large")
            }
            // Refuse bytes that do not decode as an image: nothing undecodable is written.
            guard let image = NSImage(data: data), image.isValid,
                  image.representations.first.map({ $0.pixelsWide > 0 && $0.pixelsHigh > 0 }) ?? false
            else {
                return errorReply(code: ReplyCode.notSupported)
            }
            pasteboard.clearContents()
            pasteboard.writeObjects([image])
            return written
        }

        return errorReply(code: ReplyCode.internalError)
    }

    // MARK: - Helpers

    /// PNG bytes of an image, through its TIFF representation.
    private static func pngData(from image: NSImage) -> Data? {
        guard let tiff = image.tiffRepresentation,
              let rep = NSBitmapImageRep(data: tiff) else { return nil }
        return rep.representation(using: .png, properties: [:])
    }

    /// Build the `clipboard` reply by hand: an absent value must be `null`, not a missing key.
    private static func clipboardReply(text: String?, url: String?, imagePng: Data?) -> String {
        func quoted(_ s: String?) -> String {
            guard let s = s else { return "null" }
            return "\"\(jsonEscapeString(s))\""
        }
        let image = imagePng.map { $0.base64EncodedString() }
        return "{\"kind\":\"clipboard\",\"text\":\(quoted(text)),\"url\":\(quoted(url)),"
            + "\"image_base64\":\(quoted(image)),\"bytes\":\(imagePng?.count ?? 0)}"
    }
}

// MARK: - JSON string escaping

/// Escape a string for safe embedding inside a JSON string value.
/// Handles backslash, double-quote, newline, carriage return, tab, and control characters.
private func jsonEscapeString(_ s: String) -> String {
    var result = ""
    result.reserveCapacity(s.count)
    for ch in s {
        switch ch {
        case "\\": result += "\\\\"
        case "\"": result += "\\\""
        case "\n": result += "\\n"
        case "\r": result += "\\r"
        case "\t": result += "\\t"
        default:
            if ch.asciiValue != nil && ch.asciiValue! < 0x20 {
                result += String(format: "\\u%04x", ch.asciiValue!)
            } else {
                result.append(ch)
            }
        }
    }
    return result
}
