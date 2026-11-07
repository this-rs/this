// Share sheet on macOS, using NSSharingServicePicker.
//
// Raw primitive: no consent is checked here. The access decision belongs to the device
// broker (`this_platform::device_bridge::gate`), see lib.rs.
//
// On macOS the share picker is a popover anchored to a view, not a presented controller.
// Since we are called from a Rust FFI thread (no Tauri Invoke), we dispatch to the main
// thread and block with a semaphore until the picker completes or is dismissed.
//
// Wire: matches the iOS clipboard_share.swift shape for `shared`.

import AppKit
import Foundation

enum MacShare {
    /// Present the system share picker with `text` as the only item.
    /// Returns `{"kind":"shared","completed":true|false}` or an error.
    static func shareText(_ args: [String: Any]) -> String {
        guard let text = args["text"] as? String else {
            return errorReply(code: ReplyCode.internalError)
        }

        // 5 minutes for the person to interact with the share sheet.
        return blockingCall(timeout: 300) { done in
            DispatchQueue.main.async {
                // Find the key window and a view to anchor the popover to.
                guard let window = NSApp.keyWindow ?? NSApp.windows.first(where: { $0.isVisible }),
                      let contentView = window.contentView
                else {
                    done("{\"kind\":\"shared\",\"completed\":false}")
                    return
                }

                let picker = NSSharingServicePicker(items: [text])
                let delegate = ShareDelegate(completion: done)
                // Keep the delegate alive until the picker finishes.
                ShareDelegate.active = delegate
                picker.delegate = delegate

                // Anchor at the top-centre of the content view.
                let anchor = NSRect(
                    x: contentView.bounds.midX - 1,
                    y: contentView.bounds.maxY - 1,
                    width: 2,
                    height: 2
                )
                picker.show(relativeTo: anchor, of: contentView, preferredEdge: .minY)
            }
        }
    }

    /// Present the system share picker with exactly ONE of `text`, `url`, `image_base64`,
    /// `path`. Zero content -> `missing_argument`; several, or an unusable one -> `not_supported`.
    ///
    /// Reply: `{"kind":"shared","completed":true}` as soon as the picker is shown.
    /// HONEST LIMIT: macOS gives no reliable close callback for a picker shown this way, so
    /// `completed:true` means "the picker was presented", NOT "the person shared something".
    ///
    /// Threading: Rust calls us from a `spawn_blocking` worker, never from the main thread, so
    /// `DispatchQueue.main.sync` cannot deadlock (the main thread is free to run the block).
    /// If we ever ARE on the main thread, run inline: `main.sync` from main would deadlock.
    static func shareItem(_ args: [String: Any]) -> String {
        let keys = ["text", "url", "image_base64", "path"]
        let given = keys.filter { args[$0] != nil }
        if given.isEmpty { return errorReply(code: "missing_argument") }
        if given.count > 1 { return errorReply(code: ReplyCode.notSupported) }

        let item: Any
        switch given[0] {
        case "text":
            guard let text = args["text"] as? String else { return errorReply(code: ReplyCode.notSupported) }
            item = text
        case "url":
            guard let raw = args["url"] as? String, let url = URL(string: raw) else {
                return errorReply(code: ReplyCode.notSupported)
            }
            item = url
        case "image_base64":
            guard let b64 = args["image_base64"] as? String,
                  let data = Data(base64Encoded: b64),
                  let image = NSImage(data: data)
            else { return errorReply(code: ReplyCode.notSupported) }
            item = image
        default: // "path"
            guard let path = args["path"] as? String,
                  FileManager.default.fileExists(atPath: path)
            else { return errorReply(code: ReplyCode.notSupported) }
            item = URL(fileURLWithPath: path)
        }

        func present(_ item: Any) -> String {
            guard let window = NSApp.keyWindow ?? NSApp.mainWindow ?? NSApp.windows.first,
                  let contentView = window.contentView
            else { return errorReply(code: ReplyCode.unavailable) }
            let picker = NSSharingServicePicker(items: [item])
            let anchor = NSRect(
                x: contentView.bounds.midX - 1,
                y: contentView.bounds.maxY - 1,
                width: 2,
                height: 2
            )
            picker.show(relativeTo: anchor, of: contentView, preferredEdge: .minY)
            return "{\"kind\":\"shared\",\"completed\":true}"
        }

        if Thread.isMainThread { return present(item) }
        return DispatchQueue.main.sync { present(item) }
    }
}

/// Delegate that detects when the share picker completes or is dismissed.
private final class ShareDelegate: NSObject, NSSharingServicePickerDelegate {
    /// Prevent the delegate from being deallocated while the picker is shown.
    static var active: ShareDelegate?

    private let completion: (String) -> Void
    private var finished = false

    init(completion: @escaping (String) -> Void) {
        self.completion = completion
        super.init()
    }

    func sharingServicePicker(
        _ sharingServicePicker: NSSharingServicePicker,
        didChoose service: NSSharingService?
    ) {
        guard !finished else { return }
        finished = true
        ShareDelegate.active = nil

        if service != nil {
            // The person chose a service. We report completed = true. We cannot know if they
            // actually finished (NSSharingService has no completion callback for all types),
            // but choosing a service is the closest macOS equivalent of iOS's
            // completionWithItemsHandler returning true.
            completion("{\"kind\":\"shared\",\"completed\":true}")
        } else {
            // The picker was dismissed without choosing.
            completion("{\"kind\":\"shared\",\"completed\":false}")
        }
    }
}
