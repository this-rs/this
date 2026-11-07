// Photos, read-only: the system picker (PHPicker, no permission) and the photo library status
// and request (PhotoKit, a permission). The app only ever receives what the person chose.
//
// Commands (Rust calls them by these names, see `run_mobile_plugin`):
//   photosPick(limit)  -> picked | status | error   (no permission, no key)
//   photosStatus()     -> status                    (asks nothing)
//   photosRequest()    -> status                    (asks the whole library)
//
// The photo library is NOT listed here: only the status and the explicit selection.
//
// Storage: the picked images stay in memory for the duration of the call. They are returned
// base64-encoded in the reply and written to no file. Nothing is kept once the reply is sent.
//
// Metadata: the bytes are the ones the system hands over. EXIF data, including a GPS location
// when the photo has one, is NOT removed here. See the report.

import Foundation
import Photos
import PhotosUI
import Tauri
import UIKit

/// The number of photos the person may pick at once. Mirrored by `families/photos.rs`.
enum PhotosLimit {
  static let min = 1
  static let max = 10
}

struct PhotosPickArgs: Decodable {
  let limit: Int
}

/// The error codes, as the Rust side names them (`DeviceError`, snake_case).
enum PhotosCode {
  static let unavailable = "unavailable"
  static let notSupported = "not_supported"
  static let internalError = "internal"
}

/// The size bound of a transfer, mirrored by `families/photos.rs`.
///
/// A photo taken on an iPhone is 3 to 12 MB, and base64 adds a third on top. Ten of them in one
/// answer is 40 to 160 MB crossing the IPC bridge, which the shell would then hold in memory
/// twice (JSON string, then decoded value). So an image is **shrunk** below the bound before it
/// is encoded, and the Rust side refuses a batch that is still too heavy: a refusal the caller
/// can see, rather than a silent truncation or an out-of-memory kill.
enum PhotoBytes {
  /// Per image, as the bytes the caller receives (before base64).
  static let maxImage = 2 * 1024 * 1024
  /// Per answer, all images together.
  static let maxTotal = 12 * 1024 * 1024
  /// JPEG quality used when an image has to be shrunk.
  static let quality: CGFloat = 0.8
}

/// One picked image: its MIME type (only `image/jpeg` or `image/png`) and its bytes, base64.
struct PickedImage: Encodable {
  let mime: String
  let data: String

  /// The image, when its bytes are JPEG or PNG (checked by signature). Anything else is refused.
  static func from(_ bytes: Data) -> PickedImage? {
    guard bytes.starts(with: [0xFF, 0xD8, 0xFF]) || bytes.starts(with: [0x89, 0x50, 0x4E, 0x47])
    else {
      return nil
    }
    let bytes = shrink(bytes)
    // Still too heavy after shrinking: refused rather than sent. The Rust side refuses it too,
    // so a change here cannot widen what crosses the bridge.
    guard bytes.count <= PhotoBytes.maxImage else { return nil }
    if bytes.starts(with: [0xFF, 0xD8, 0xFF]) {
      return PickedImage(mime: "image/jpeg", data: bytes.base64EncodedString())
    }
    return PickedImage(mime: "image/png", data: bytes.base64EncodedString())
  }

  /// The same image, re-encoded as JPEG below the per-image bound. An image already small
  /// enough is returned untouched, bytes for bytes.
  static func shrink(_ bytes: Data) -> Data {
    guard bytes.count > PhotoBytes.maxImage, let image = UIImage(data: bytes) else { return bytes }
    if let jpeg = image.jpegData(compressionQuality: PhotoBytes.quality),
      jpeg.count <= PhotoBytes.maxImage
    {
      return jpeg
    }
    // Quality alone is not enough for a very large photo: halve the pixels until it fits.
    var scaled = image
    while scaled.size.width > 64 {
      let size = CGSize(width: scaled.size.width / 2, height: scaled.size.height / 2)
      let format = UIGraphicsImageRendererFormat.default()
      format.scale = 1
      scaled = UIGraphicsImageRenderer(size: size, format: format).image { _ in
        scaled.draw(in: CGRect(origin: .zero, size: size))
      }
      if let jpeg = scaled.jpegData(compressionQuality: PhotoBytes.quality),
        jpeg.count <= PhotoBytes.maxImage
      {
        return jpeg
      }
    }
    return bytes
  }
}

/// The answers of the photos bridge. Same shape as `Reply.swift` (`kind` first), kept here
/// because the device contract has no photos answer yet.
enum PhotosReply: Encodable {
  case picked([PickedImage])
  case status(state: String)
  case error(code: String)

  private enum Key: String, CodingKey {
    case kind, items, state, code
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    switch self {
    case let .picked(items):
      try c.encode("picked", forKey: .kind)
      try c.encode(items, forKey: .items)
    case let .status(state):
      try c.encode("status", forKey: .kind)
      try c.encode(state, forKey: .state)
    case let .error(code):
      try c.encode("error", forKey: .kind)
      try c.encode(code, forKey: .code)
    }
  }
}

extension ThisDevicePlugin {
  /// Presents the system picker and answers with the chosen images. A cancel answers `picked`
  /// with no items: it is not an error.
  @objc public func photosPick(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(PhotosPickArgs.self)
    guard (PhotosLimit.min...PhotosLimit.max).contains(args.limit) else {
      // The Rust side checks the bound first; a value out of range here is a bug, not a choice.
      invoke.resolve(PhotosReply.error(code: PhotosCode.internalError))
      return
    }
    guard #available(iOS 14, *) else {
      invoke.resolve(PhotosReply.error(code: PhotosCode.notSupported))
      return
    }
    PhotoPickSession.start(invoke, limit: args.limit, presenter: manager.viewController)
  }

  /// The photo library permission, read without prompting the person.
  @objc public func photosStatus(_ invoke: Invoke) {
    guard #available(iOS 14, *) else {
      invoke.resolve(PhotosReply.status(state: "unsupported"))
      return
    }
    invoke.resolve(PhotosReply.status(state: PhotoLibraryState.name(
      PHPhotoLibrary.authorizationStatus(for: .readWrite))))
  }

  /// Asks the person for the whole photo library (read/write access level, as PhotoKit names it).
  @objc public func photosRequest(_ invoke: Invoke) {
    guard #available(iOS 14, *) else {
      invoke.resolve(PhotosReply.status(state: "unsupported"))
      return
    }
    // iOS ends the app at the request when the usage text is missing: refuse instead.
    guard Bundle.main.object(forInfoDictionaryKey: "NSPhotoLibraryUsageDescription") != nil else {
      invoke.resolve(PhotosReply.error(code: PhotosCode.internalError))
      return
    }
    PHPhotoLibrary.requestAuthorization(for: .readWrite) { status in
      invoke.resolve(PhotosReply.status(state: PhotoLibraryState.name(status)))
    }
  }
}

/// The photo library permission, in the contract's words. `limited` is the iOS 14 partial
/// access: the app sees only the photos the person selected, and says so.
@available(iOS 14, *)
enum PhotoLibraryState {
  static func name(_ status: PHAuthorizationStatus) -> String {
    switch status {
    case .authorized: return "granted"
    case .limited: return "limited"
    case .denied, .restricted: return "denied"
    // Not asked yet: the system asks when the app requests access.
    case .notDetermined: return "prompt"
    @unknown default: return "prompt"
    }
  }
}

/// One picker presentation. It keeps itself alive until it answers: the picker holds its
/// delegate weakly.
@available(iOS 14, *)
final class PhotoPickSession: NSObject, PHPickerViewControllerDelegate {
  // One picker at a time: a second call while one is open is refused, not queued.
  private static var active: [ObjectIdentifier: PhotoPickSession] = [:]

  private let invoke: Invoke
  private var finished = false

  private init(invoke: Invoke) {
    self.invoke = invoke
  }

  /// Presents the picker on the main thread. Called from the plugin's IPC queue.
  static func start(_ invoke: Invoke, limit: Int, presenter: UIViewController?) {
    DispatchQueue.main.async {
      guard PhotoPickSession.active.isEmpty else {
        invoke.resolve(PhotosReply.error(code: PhotosCode.unavailable))
        return
      }
      guard let presenter = presenter else {
        invoke.resolve(PhotosReply.error(code: PhotosCode.unavailable))
        return
      }
      var config = PHPickerConfiguration()
      config.filter = .images
      config.selectionLimit = limit
      // Asks the system for the most compatible representation: JPEG for HEIC originals.
      config.preferredAssetRepresentationMode = .compatible
      let picker = PHPickerViewController(configuration: config)
      let session = PhotoPickSession(invoke: invoke)
      picker.delegate = session
      PhotoPickSession.active[ObjectIdentifier(session)] = session
      presenter.present(picker, animated: true)
    }
  }

  func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
    picker.dismiss(animated: true)
    guard !results.isEmpty else {
      // Cancelled: nothing chosen, which is an answer, not a failure.
      finish(.picked([]))
      return
    }

    let group = DispatchGroup()
    let lock = NSLock()
    var images = [PickedImage?](repeating: nil, count: results.count)
    var refused = false

    for (index, result) in results.enumerated() {
      group.enter()
      result.itemProvider.loadDataRepresentation(forTypeIdentifier: "public.image") { data, _ in
        let image = data.flatMap { PickedImage.from($0) }
        lock.lock()
        if let image = image {
          images[index] = image
        } else {
          refused = true
        }
        lock.unlock()
        group.leave()
      }
    }

    group.notify(queue: .main) {
      // One image that is neither JPEG nor PNG fails the whole call: no partial selection.
      if refused {
        self.finish(.error(code: PhotosCode.internalError))
      } else {
        self.finish(.picked(images.compactMap { $0 }))
      }
    }
  }

  private func finish(_ reply: PhotosReply) {
    guard !finished else { return }
    finished = true
    invoke.resolve(reply)
    PhotoPickSession.active[ObjectIdentifier(self)] = nil
  }
}
