// Files the person picks (`Capability::FilesPick`), in two commands.
//
// `filesPick`: the system document picker (UIDocumentPickerViewController). The person chooses the
// files; nothing is listed, and no permission or Info.plist key is needed. Each chosen file comes
// back with a security-scoped bookmark, so the caller can read it again after a restart.
//
// `filesRead`: resolves a bookmark, opens that one file, and reads at most
// `FilesLimits.readMaxBytes`. No directory is listed and no other path is touched.
//
// Sensitivity: a bookmark is a durable grant of access to one file. It is never logged, and this
// plugin stores none: it only hands the bookmark to the caller, which keeps it (in the Obrain kernel).

import Foundation
import Tauri
import UIKit
import UniformTypeIdentifiers

/// The bounds of the bridge. The Rust side checks the same numbers (`families::files`).
enum FilesLimits {
  static let pickMin = 1
  static let pickMax = 5
  /// Largest file `filesRead` returns: 10 MiB.
  static let readMaxBytes = 10 * 1024 * 1024
}

/// Error codes specific to files, named as the Rust side does (`families::files::FilesError`).
enum FilesCode {
  static let notAuthorized = "not_authorized"
  static let notSupported = "not_supported"
  static let unavailable = "unavailable"
  static let invalidBookmark = "invalid_bookmark"
  static let tooLarge = "too_large"
  static let internalError = "internal"
}

struct FilesPickArgs: Decodable {
  let max: Int?
}

struct FilesReadArgs: Decodable {
  let bookmark: String
}

/// One file the person picked. `bookmark` is base64 of `URL.bookmarkData`: sensitive, never log it.
struct PickedFile: Encodable {
  let id: String
  let name: String
  let size: Int64
  let bookmark: String
}

/// The answer to `filesPick`: `{"kind":"files","files":[...]}`. Empty when the person cancels.
struct FilesAnswer: Encodable {
  let files: [PickedFile]

  private enum Key: String, CodingKey {
    case kind, files
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    try c.encode("files", forKey: .kind)
    try c.encode(files, forKey: .files)
  }
}

/// The answer to `filesRead`: `{"kind":"file","name":..,"size":..,"stale":..,"bookmark":..|null,"data":"<base64>"}`.
/// `bookmark` is set only when the stored bookmark was stale and a fresh one could be made: the
/// caller must replace the stored bookmark with it. `stale` is true whenever iOS flagged it.
struct FileContent: Encodable {
  let name: String
  let size: Int64
  let stale: Bool
  let bookmark: String?
  let data: String

  private enum Key: String, CodingKey {
    case kind, name, size, stale, bookmark, data
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    try c.encode("file", forKey: .kind)
    try c.encode(name, forKey: .name)
    try c.encode(size, forKey: .size)
    try c.encode(stale, forKey: .stale)
    // `encode` (not `encodeIfPresent`) writes `null` when there is no fresh bookmark.
    try c.encode(bookmark, forKey: .bookmark)
    try c.encode(data, forKey: .data)
  }
}

/// One pick, from presentation to answer. Kept alive until it answers: the caller only holds the `Invoke`.
final class FilePickSession: NSObject, UIDocumentPickerDelegate {
  private static var active: [ObjectIdentifier: FilePickSession] = [:]

  private let invoke: Invoke
  private let limit: Int
  private var finished = false

  private init(_ invoke: Invoke, limit: Int) {
    self.invoke = invoke
    self.limit = limit
    super.init()
  }

  /// Presents the system picker. Called on the main thread with a `limit` already in 1...5.
  static func present(_ invoke: Invoke, limit: Int, from presenter: UIViewController) {
    guard #available(iOS 14.0, *) else {
      // The picker by content type (UTType) needs iOS 14.
      invoke.resolve(Reply.error(code: FilesCode.notSupported))
      return
    }
    guard presenter.presentedViewController == nil else {
      // Another sheet is already on screen: a second picker would be a stacked, unanswerable one.
      invoke.resolve(Reply.error(code: FilesCode.unavailable))
      return
    }
    let session = FilePickSession(invoke, limit: limit)
    active[ObjectIdentifier(session)] = session
    // Bounded types: PDF, plain text, JSON and CSV. Nothing else can be picked.
    let picker = UIDocumentPickerViewController(
      forOpeningContentTypes: [.pdf, .plainText, .json, .commaSeparatedText])
    picker.allowsMultipleSelection = limit > 1
    picker.delegate = session
    presenter.present(picker, animated: true)
  }

  func documentPicker(
    _ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]
  ) {
    // The picker may return more than the bound: only the first `limit` are opened. The others
    // never get an access, so they are not touched.
    var files: [PickedFile] = []
    for url in urls.prefix(limit) {
      guard let file = FilePickSession.describe(url) else {
        // A file without a bookmark cannot be read again later: the pick fails as a whole.
        finish(Reply.error(code: FilesCode.unavailable))
        return
      }
      files.append(file)
    }
    finish(FilesAnswer(files: files))
  }

  func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
    finish(FilesAnswer(files: []))
  }

  /// The name, size and bookmark of one chosen file. The access is open only while they are read,
  /// and released on every path. Nil when no bookmark can be made.
  static func describe(_ url: URL) -> PickedFile? {
    let scoped = url.startAccessingSecurityScopedResource()
    defer {
      if scoped { url.stopAccessingSecurityScopedResource() }
    }
    guard
      let bookmark = try? url.bookmarkData(
        options: [], includingResourceValuesForKeys: nil, relativeTo: nil)
    else {
      return nil
    }
    let size = (try? url.resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0
    return PickedFile(
      id: UUID().uuidString,
      name: url.lastPathComponent,
      size: Int64(size),
      bookmark: bookmark.base64EncodedString())
  }

  private func finish<T: Encodable>(_ answer: T) {
    guard !finished else { return }
    finished = true
    invoke.resolve(answer)
    FilePickSession.active[ObjectIdentifier(self)] = nil
  }
}

/// Reads one file again from its bookmark. Runs off the main thread: the file can be large.
enum FileRead {
  static func run(_ invoke: Invoke, bookmark: String) {
    guard let raw = Data(base64Encoded: bookmark), !raw.isEmpty else {
      invoke.resolve(Reply.error(code: FilesCode.invalidBookmark))
      return
    }
    var stale = false
    guard
      let url = try? URL(
        resolvingBookmarkData: raw, options: [], relativeTo: nil, bookmarkDataIsStale: &stale)
    else {
      invoke.resolve(Reply.error(code: FilesCode.invalidBookmark))
      return
    }
    let scoped = url.startAccessingSecurityScopedResource()
    defer {
      if scoped { url.stopAccessingSecurityScopedResource() }
    }

    // A directory is never read, and a file announced above the bound is refused before any read.
    let values = try? url.resourceValues(forKeys: [.isDirectoryKey, .fileSizeKey])
    if values?.isDirectory == true {
      invoke.resolve(Reply.error(code: FilesCode.invalidBookmark))
      return
    }
    if let size = values?.fileSize, size > FilesLimits.readMaxBytes {
      invoke.resolve(Reply.error(code: FilesCode.tooLarge))
      return
    }

    guard let handle = try? FileHandle(forReadingFrom: url) else {
      invoke.resolve(Reply.error(code: FilesCode.notAuthorized))
      return
    }
    defer {
      try? handle.close()
    }
    // One byte past the bound: a file that grew since its size was read is refused too.
    let content = handle.readData(ofLength: FilesLimits.readMaxBytes + 1)
    if content.count > FilesLimits.readMaxBytes {
      invoke.resolve(Reply.error(code: FilesCode.tooLarge))
      return
    }

    // A stale bookmark still resolved: the file is read, and a fresh bookmark is returned so the
    // caller can replace the stored one. It is made while the access is open.
    var fresh: String?
    if stale,
      let renewed = try? url.bookmarkData(
        options: [], includingResourceValuesForKeys: nil, relativeTo: nil)
    {
      fresh = renewed.base64EncodedString()
    }

    invoke.resolve(
      FileContent(
        name: url.lastPathComponent,
        size: Int64(content.count),
        stale: stale,
        bookmark: fresh,
        data: content.base64EncodedString()))
  }
}

extension ThisDevicePlugin {
  /// Presents the system picker. `max` is the number of files allowed, clamped to 1...5.
  @objc public func filesPick(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(FilesPickArgs.self)
    let limit = Swift.min(
      Swift.max(args.max ?? FilesLimits.pickMin, FilesLimits.pickMin), FilesLimits.pickMax)
    guard let presenter = manager.viewController else {
      invoke.resolve(Reply.error(code: FilesCode.unavailable))
      return
    }
    FilePickSession.present(invoke, limit: limit, from: presenter)
  }

  /// Reads the file a bookmark designates, at most `FilesLimits.readMaxBytes`.
  @objc public func filesRead(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(FilesReadArgs.self)
    DispatchQueue.global(qos: .userInitiated).async {
      FileRead.run(invoke, bookmark: args.bookmark)
    }
  }
}
