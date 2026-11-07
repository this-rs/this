// Files the person picks on macOS, using NSOpenPanel.
//
// `filesPick`: the system open panel. The person chooses the files; no permission or
// Info.plist key is needed. Each chosen file comes back with a security-scoped bookmark
// (`.withSecurityScope` on macOS), so the caller can read it again after a restart.
//
// `filesRead`: resolves a bookmark, opens that one file, and reads at most 10 MiB.
// No directory is listed and no other path is touched.
//
// Sensitivity: a bookmark is a durable grant of access to one file. It is never logged.
//
// Wire: matches the iOS files.swift shapes exactly.

import AppKit
import Foundation
import UniformTypeIdentifiers

// MARK: - Limits (same as iOS)

private enum FilesLimits {
    static let pickMin = 1
    static let pickMax = 5
    /// Largest file `filesRead` returns: 10 MiB.
    static let readMaxBytes = 10 * 1024 * 1024
}

private enum FilesCode {
    static let notAuthorized  = "not_authorized"
    static let unavailable    = "unavailable"
    static let invalidBookmark = "invalid_bookmark"
    static let tooLarge       = "too_large"
    static let internalError  = "internal"
}

enum MacFiles {
    // MARK: - Pick

    /// Present NSOpenPanel on the main thread. Block until the person picks or cancels.
    /// Returns `{"kind":"files","files":[...]}`.
    static func pick(_ args: [String: Any]) -> String {
        let maxRaw = args["max"] as? Int ?? FilesLimits.pickMin
        let limit = min(max(maxRaw, FilesLimits.pickMin), FilesLimits.pickMax)

        // 5 minutes for the person to browse and choose files.
        return blockingCall(timeout: 300) { done in
            DispatchQueue.main.async {
                let panel = NSOpenPanel()
                panel.canChooseFiles = true
                panel.canChooseDirectories = false
                panel.allowsMultipleSelection = limit > 1
                panel.allowedContentTypes = [.pdf, .plainText, .json, .commaSeparatedText]

                panel.begin { response in
                    guard response == .OK else {
                        // Cancelled: empty list, not an error.
                        done("{\"kind\":\"files\",\"files\":[]}")
                        return
                    }

                    var files: [[String: Any]] = []
                    for url in panel.urls.prefix(limit) {
                        guard let entry = describeFile(url) else {
                            done(errorReply(code: FilesCode.unavailable))
                            return
                        }
                        files.append(entry)
                    }

                    done(encodeDictJson(["kind": "files", "files": files]))
                }
            }
        }
    }

    // MARK: - Read

    /// Resolve a bookmark and read the file, at most 10 MiB.
    /// Returns `{"kind":"file","name":..,"size":..,"stale":..,"bookmark":..|null,"data":"base64"}`.
    static func read(_ args: [String: Any]) -> String {
        guard let bookmarkB64 = args["bookmark"] as? String,
              let raw = Data(base64Encoded: bookmarkB64), !raw.isEmpty
        else {
            return errorReply(code: FilesCode.invalidBookmark)
        }

        var stale = false
        guard let url = try? URL(
            resolvingBookmarkData: raw,
            options: [.withSecurityScope],
            relativeTo: nil,
            bookmarkDataIsStale: &stale
        ) else {
            return errorReply(code: FilesCode.invalidBookmark)
        }

        let scoped = url.startAccessingSecurityScopedResource()
        defer {
            if scoped { url.stopAccessingSecurityScopedResource() }
        }

        // Refuse directories.
        let values = try? url.resourceValues(forKeys: [.isDirectoryKey, .fileSizeKey])
        if values?.isDirectory == true {
            return errorReply(code: FilesCode.invalidBookmark)
        }
        if let size = values?.fileSize, size > FilesLimits.readMaxBytes {
            return errorReply(code: FilesCode.tooLarge)
        }

        guard let handle = try? FileHandle(forReadingFrom: url) else {
            return errorReply(code: FilesCode.notAuthorized)
        }
        defer { try? handle.close() }

        // One byte past the bound: a file that grew is refused too.
        let content = handle.readData(ofLength: FilesLimits.readMaxBytes + 1)
        if content.count > FilesLimits.readMaxBytes {
            return errorReply(code: FilesCode.tooLarge)
        }

        // Renew a stale bookmark while the access is open.
        var freshBookmark: String? = nil
        if stale,
           let renewed = try? url.bookmarkData(
               options: [.withSecurityScope],
               includingResourceValuesForKeys: nil,
               relativeTo: nil
           )
        {
            freshBookmark = renewed.base64EncodedString()
        }

        let result: [String: Any] = [
            "kind": "file",
            "name": url.lastPathComponent,
            "size": content.count,
            "stale": stale,
            "bookmark": freshBookmark as Any,
            "data": content.base64EncodedString(),
        ]
        return encodeDictJson(result)
    }

    // MARK: - Write

    /// Largest payload `filesWrite` accepts: 8 MiB (same bound as the Rust side).
    private static let writeMaxBytes = 8 * 1024 * 1024

    /// Write a named file in the app's temporary directory.
    /// Returns `{"kind":"saved","path":..,"bytes":N}`.
    ///
    /// The name must be a bare file name: no separator, no `.`/`..`, no control character.
    /// A direct native call cannot aim at an arbitrary path.
    static func write(_ args: [String: Any]) -> String {
        guard let rawName = args["name"] as? String else {
            return errorReply(code: "missing_argument")
        }
        let name = rawName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else {
            return errorReply(code: "missing_argument")
        }
        let hasControl = name.unicodeScalars.contains { $0.value < 0x20 || $0.value == 0x7F }
        guard !name.contains("/"), !name.contains("\\"),
              name != ".", name != "..", !hasControl, name.utf8.count <= 255 else {
            return errorReply(code: FilesCode.internalError)
        }
        guard let b64 = args["data_base64"] as? String,
              let data = Data(base64Encoded: b64) else {
            return errorReply(code: "bad_base64")
        }
        guard data.count <= writeMaxBytes else {
            return errorReply(code: FilesCode.tooLarge)
        }
        let dir = FileManager.default.temporaryDirectory
        let url = dir.appendingPathComponent(name, isDirectory: false)
        // Belt and braces: the resolved file must still be a direct child of the directory.
        guard url.deletingLastPathComponent().standardizedFileURL.path
                == dir.standardizedFileURL.path else {
            return errorReply(code: FilesCode.internalError)
        }
        do {
            try data.write(to: url, options: .atomic)
            return encodeDictJson(["kind": "saved", "path": url.path, "bytes": data.count])
        } catch {
            // The name may designate an existing folder, or the volume may be full.
            return errorReply(code: FilesCode.internalError)
        }
    }

    // MARK: - Helpers

    /// Describe one picked file: name, size, and a security-scoped bookmark.
    private static func describeFile(_ url: URL) -> [String: Any]? {
        let scoped = url.startAccessingSecurityScopedResource()
        defer {
            if scoped { url.stopAccessingSecurityScopedResource() }
        }

        guard let bookmark = try? url.bookmarkData(
            options: [.withSecurityScope],
            includingResourceValuesForKeys: nil,
            relativeTo: nil
        ) else {
            return nil
        }

        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0

        return [
            "id": UUID().uuidString,
            "name": url.lastPathComponent,
            "size": Int64(size),
            "bookmark": bookmark.base64EncodedString(),
        ]
    }
}
