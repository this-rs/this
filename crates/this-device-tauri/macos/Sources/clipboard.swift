// Clipboard (text only) on macOS, using NSPasteboard.
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
    /// Read the text on the general pasteboard.
    /// Returns `{"kind":"clipboard_text","text":"..."}` or `{"kind":"clipboard_text","text":null}`.
    static func read() -> String {
        let pasteboard = NSPasteboard.general

        if let text = pasteboard.string(forType: .string) {
            // Escape the text for JSON embedding.
            let escaped = jsonEscapeString(text)
            return "{\"kind\":\"clipboard_text\",\"text\":\"\(escaped)\"}"
        }

        // Check if the pasteboard is empty vs holds non-text content.
        if pasteboard.pasteboardItems?.isEmpty ?? true {
            return "{\"kind\":\"clipboard_text\",\"text\":null}"
        }

        // Has items but none are text: not supported (image, file, etc.).
        return errorReply(code: ReplyCode.notSupported)
    }

    /// Write text to the general pasteboard.
    /// Returns `{"kind":"clipboard_written"}`.
    static func write(_ args: [String: Any]) -> String {
        guard let text = args["text"] as? String else {
            return errorReply(code: ReplyCode.internalError)
        }

        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
        return "{\"kind\":\"clipboard_written\"}"
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
