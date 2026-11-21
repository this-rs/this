// Photos: permission status, request, and system picker on macOS.
//
// macOS 13+: PHPickerViewController from PhotosUI (same as iOS 14+, no permission needed).
// macOS 12+: PHPhotoLibrary.requestAuthorization(for:) for full library access.
// macOS 10.13+: PHPhotoLibrary.authorizationStatus(for:) for status.
//
// The picker does not need photo library access (the person chooses what to share).
// The status/request commands are for full library access (PhotoKit).
//
// Images are compressed to JPEG below 2 MiB before encoding to base64.

import AppKit
import Foundation
import Photos
import PhotosUI

enum MacPhotos {
    // MARK: - Limits (mirror families/photos.rs)

    static let pickMin = 1
    static let pickMax = 10
    /// Per image, as the bytes the caller receives (before base64).
    static let maxImageBytes = 2 * 1024 * 1024
    /// JPEG quality used when an image has to be shrunk.
    static let quality: CGFloat = 0.8

    // MARK: - Permission state

    /// The photo library permission in the contract's words.
    static func stateName(_ status: PHAuthorizationStatus) -> String {
        switch status {
        case .authorized: return Etat.granted.fil
        case .limited: return Etat.limited.fil
        case .denied, .restricted: return Etat.denied.fil
        case .notDetermined: return Etat.prompt.fil
        @unknown default: return Etat.prompt.fil
        }
    }

    /// Current state for `macos_device_status`.
    static func currentState() -> String {
        return stateName(PHPhotoLibrary.authorizationStatus(for: .readWrite))
    }

    // MARK: - Commands

    /// The photo library permission, read without prompting.
    static func status() -> String {
        return statusReply(state: currentState())
    }

    /// Asks the person for the whole photo library (readWrite).
    static func request() -> String {
        return blockingCall(timeout: 120) { done in
            PHPhotoLibrary.requestAuthorization(for: .readWrite) { status in
                done(statusReply(state: stateName(status)))
            }
        }
    }

    /// Presents the system photo picker. macOS 13+ only (PHPickerViewController).
    /// Returns picked images as base64 JPEG.
    static func pick(_ args: [String: Any]) -> String {
        let limit = (args["limit"] as? Int) ?? pickMin
        guard (pickMin...pickMax).contains(limit) else {
            return errorReply(code: ReplyCode.internalError)
        }

        guard #available(macOS 13.0, *) else {
            return errorReply(code: ReplyCode.notSupported)
        }

        // PHPickerViewController must be presented on the main thread.
        return blockingCall(timeout: 300) { done in
            DispatchQueue.main.async {
                presentPicker(limit: limit, completion: done)
            }
        }
    }

    // MARK: - List / read (PhotoKit, same shapes as iOS)

    /// Library access sufficient to read assets: full or limited.
    private static func canReadLibrary() -> Bool {
        let status = PHPhotoLibrary.authorizationStatus(for: .readWrite)
        return status == .authorized || status == .limited
    }

    private static func int64Arg(_ args: [String: Any], _ key: String) -> Int64? {
        if let v = args[key] as? Int64 { return v }
        if let v = args[key] as? Int { return Int64(v) }
        if let v = args[key] as? Double { return Int64(v) }
        return nil
    }

    /// `photosList` — photos taken inside the window, newest first.
    static func list(_ args: [String: Any]) -> String {
        guard let startMs = int64Arg(args, "start_ms"),
              let endMs = int64Arg(args, "end_ms"),
              let limit = int64Arg(args, "limit"),
              limit > 0, limit <= 100, endMs > startMs else {
            return errorReply(code: ReplyCode.internalError)
        }
        guard canReadLibrary() else {
            return errorReply(code: ReplyCode.notAuthorized)
        }
        // The predicate carries the window: PhotoKit returns only what is asked for.
        let options = PHFetchOptions()
        options.predicate = NSPredicate(
            format: "mediaType == %d AND creationDate >= %@ AND creationDate <= %@",
            PHAssetMediaType.image.rawValue,
            Date(timeIntervalSince1970: Double(startMs) / 1000) as NSDate,
            Date(timeIntervalSince1970: Double(endMs) / 1000) as NSDate
        )
        options.sortDescriptors = [NSSortDescriptor(key: "creationDate", ascending: false)]
        options.fetchLimit = Int(limit)
        let assets = PHAsset.fetchAssets(with: options)
        var photos: [[String: Any]] = []
        assets.enumerateObjects { asset, _, _ in
            photos.append([
                "id": asset.localIdentifier,
                "created_ms": Int64(((asset.creationDate?.timeIntervalSince1970 ?? 0) * 1000).rounded()),
                "width": asset.pixelWidth,
                "height": asset.pixelHeight,
                "favorite": asset.isFavorite,
            ])
        }
        return encodeDictJson(["kind": "album", "photos": photos])
    }

    /// `photosRead` — one photo as JPEG, longest side at most `max_px` (clamped 64...4096).
    static func readPhoto(_ args: [String: Any]) -> String {
        guard let id = args["id"] as? String, !id.isEmpty else {
            return errorReply(code: ReplyCode.internalError)
        }
        let requested = int64Arg(args, "max_px") ?? 1024
        guard canReadLibrary() else {
            return errorReply(code: ReplyCode.notAuthorized)
        }
        guard let asset = PHAsset.fetchAssets(withLocalIdentifiers: [id], options: nil).firstObject else {
            // An identifier the library no longer knows (photo deleted): explicit refusal.
            return errorReply(code: ReplyCode.notSupported)
        }
        let maxPx = CGFloat(max(64, min(requested, 4096)))
        let options = PHImageRequestOptions()
        options.isSynchronous = false
        options.isNetworkAccessAllowed = true
        options.deliveryMode = .highQualityFormat
        options.resizeMode = .exact

        return blockingCall(timeout: 60) { done in
            // PhotoKit may call twice (blurry preview, then the sharp image): only the final
            // version answers, and only once.
            var answered = false
            PHImageManager.default().requestImage(
                for: asset,
                targetSize: CGSize(width: maxPx, height: maxPx),
                contentMode: .aspectFit,
                options: options
            ) { image, info in
                if let degraded = info?[PHImageResultIsDegradedKey] as? Bool, degraded { return }
                guard !answered else { return }
                answered = true
                guard let image = image,
                      let tiff = image.tiffRepresentation,
                      let rep = NSBitmapImageRep(data: tiff),
                      let jpeg = rep.representation(
                          using: .jpeg, properties: [.compressionFactor: quality])
                else {
                    done(errorReply(code: ReplyCode.unavailable))
                    return
                }
                done(encodeDictJson([
                    "kind": "photo",
                    "data_base64": jpeg.base64EncodedString(),
                    "width": rep.pixelsWide,
                    "height": rep.pixelsHigh,
                    "bytes": jpeg.count,
                ]))
            }
        }
    }

    // MARK: - Image shrinking

    /// Shrink an image to fit within maxImageBytes as JPEG.
    static func shrinkToJpeg(_ imageData: Data) -> Data? {
        guard let image = NSImage(data: imageData) else { return nil }
        guard let cgImage = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
            return nil
        }

        var width = cgImage.width
        var height = cgImage.height

        // Try at original size first.
        if let jpeg = jpegData(cgImage: cgImage, width: width, height: height, quality: quality),
           jpeg.count <= maxImageBytes {
            return jpeg
        }

        // Scale down until it fits.
        while width > 64 {
            width /= 2
            height /= 2
            if let jpeg = jpegData(cgImage: cgImage, width: width, height: height, quality: quality),
               jpeg.count <= maxImageBytes {
                return jpeg
            }
        }
        return nil
    }

    /// Create JPEG data from a CGImage at a given size.
    private static func jpegData(cgImage: CGImage, width: Int, height: Int, quality: CGFloat) -> Data? {
        let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil,
            pixelsWide: width,
            pixelsHigh: height,
            bitsPerSample: 8,
            samplesPerPixel: 4,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: .deviceRGB,
            bytesPerRow: 0,
            bitsPerPixel: 0
        )
        guard let rep = rep, let ctx = NSGraphicsContext(bitmapImageRep: rep) else { return nil }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = ctx
        ctx.cgContext.draw(cgImage, in: CGRect(x: 0, y: 0, width: width, height: height))
        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .jpeg, properties: [.compressionFactor: quality])
    }
}

// MARK: - PHPicker presentation (macOS 13+)

@available(macOS 13.0, *)
private func presentPicker(limit: Int, completion: @escaping (String) -> Void) {
    var config = PHPickerConfiguration()
    config.filter = .images
    config.selectionLimit = limit
    config.preferredAssetRepresentationMode = .compatible

    let picker = PHPickerViewController(configuration: config)
    let delegate = MacPhotoPickerDelegate(completion: completion)
    picker.delegate = delegate

    // Keep delegate alive until pick completes.
    objc_setAssociatedObject(picker, "delegate", delegate, .OBJC_ASSOCIATION_RETAIN)

    // Present the picker in a new window (macOS has no UIViewController hierarchy).
    let window = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
        styleMask: [.titled, .closable, .resizable],
        backing: .buffered,
        defer: false
    )
    window.title = "Choisir des photos"
    window.contentViewController = picker
    window.center()

    // Keep the window alive.
    objc_setAssociatedObject(picker, "window", window, .OBJC_ASSOCIATION_RETAIN)

    // Watch for window close (user dismissed without picking).
    let observer = NotificationCenter.default.addObserver(
        forName: NSWindow.willCloseNotification,
        object: window,
        queue: .main
    ) { _ in
        // The delegate might already have answered; it guards against double-finish.
        delegate.windowClosed()
    }
    objc_setAssociatedObject(picker, "observer", observer, .OBJC_ASSOCIATION_RETAIN)

    window.makeKeyAndOrderFront(nil)
}

@available(macOS 13.0, *)
private final class MacPhotoPickerDelegate: NSObject, PHPickerViewControllerDelegate {
    private let completion: (String) -> Void
    private var finished = false

    init(completion: @escaping (String) -> Void) {
        self.completion = completion
    }

    func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
        // Close the window.
        if let window = picker.view.window {
            window.close()
        }

        guard !results.isEmpty else {
            finish(encodeDictJson(["kind": "picked", "items": []]))
            return
        }

        let group = DispatchGroup()
        let lock = NSLock()
        var images: [[String: Any]] = []
        var hasError = false

        for result in results {
            group.enter()
            result.itemProvider.loadDataRepresentation(forTypeIdentifier: "public.image") { data, _ in
                defer { group.leave() }
                guard let data = data else {
                    lock.lock()
                    hasError = true
                    lock.unlock()
                    return
                }
                // Check for JPEG or PNG.
                let isJpeg = data.starts(with: [0xFF, 0xD8, 0xFF])
                let isPng = data.starts(with: [0x89, 0x50, 0x4E, 0x47])
                guard isJpeg || isPng else {
                    // Try to convert to JPEG anyway (HEIC, TIFF, etc.).
                    if let jpeg = MacPhotos.shrinkToJpeg(data) {
                        lock.lock()
                        images.append([
                            "mime": "image/jpeg",
                            "data": jpeg.base64EncodedString()
                        ])
                        lock.unlock()
                    } else {
                        lock.lock()
                        hasError = true
                        lock.unlock()
                    }
                    return
                }
                // Shrink if needed.
                let final_data: Data
                if data.count > MacPhotos.maxImageBytes {
                    guard let shrunk = MacPhotos.shrinkToJpeg(data) else {
                        lock.lock()
                        hasError = true
                        lock.unlock()
                        return
                    }
                    final_data = shrunk
                } else {
                    final_data = data
                }
                let mime = final_data.starts(with: [0xFF, 0xD8, 0xFF]) ? "image/jpeg" : "image/png"
                lock.lock()
                images.append([
                    "mime": mime,
                    "data": final_data.base64EncodedString()
                ])
                lock.unlock()
            }
        }

        group.notify(queue: .main) { [self] in
            if hasError {
                finish(errorReply(code: ReplyCode.internalError))
            } else {
                finish(encodeDictJson(["kind": "picked", "items": images]))
            }
        }
    }

    func windowClosed() {
        // If the window closed without the delegate getting results, treat as cancel.
        finish(encodeDictJson(["kind": "picked", "items": []]))
    }

    private func finish(_ reply: String) {
        guard !finished else { return }
        finished = true
        completion(reply)
    }
}
