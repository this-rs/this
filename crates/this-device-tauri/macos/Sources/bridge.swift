// Shared types and helpers for the macOS device bridge.
//
// Every native function returns a JSON string allocated with strdup(). The Rust side frees it
// with libc::free(). This file defines the common reply shapes and helpers so the family files
// only carry their own logic.
//
// Wire format: identical to the iOS side (Reply.swift), so the Rust bridge decodes the same JSON
// regardless of platform.

import Foundation

// MARK: - JSON result allocation

/// Allocate a C string from a Swift string. The caller (Rust) frees it with libc::free().
func jsonResult(_ json: String) -> UnsafeMutablePointer<CChar> {
    return strdup(json)!
}

// MARK: - Reply helpers (match Reply.swift on iOS)

/// A permission status reply: `{"kind":"status","state":"<state>"}`.
func statusReply(state: String) -> String {
    return "{\"kind\":\"status\",\"state\":\"\(state)\"}"
}

/// An error reply: `{"kind":"error","code":"<code>"}`.
func errorReply(code: String) -> String {
    return "{\"kind\":\"error\",\"code\":\"\(code)\"}"
}

// MARK: - Common error codes (match DeviceError on the Rust side)

enum ReplyCode {
    static let notAuthorized = "not_authorized"
    static let notSupported  = "not_supported"
    static let unavailable   = "unavailable"
    static let timeout       = "timeout"
    static let internalError = "internal"
}

// MARK: - JSON arg decoding

/// Decode JSON args from a C string into a dictionary.
func decodeArgs(_ argsPtr: UnsafePointer<CChar>) -> [String: Any] {
    let str = String(cString: argsPtr)
    guard let data = str.data(using: .utf8),
          let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        return [:]
    }
    return json
}

/// Encode an Encodable value to a JSON string. Falls back to an error reply on failure.
func encodeJson<T: Encodable>(_ value: T) -> String {
    guard let data = try? JSONEncoder().encode(value),
          let str = String(data: data, encoding: .utf8) else {
        return errorReply(code: ReplyCode.internalError)
    }
    return str
}

/// Encode a dictionary to a JSON string.
func encodeDictJson(_ dict: [String: Any]) -> String {
    guard let data = try? JSONSerialization.data(withJSONObject: dict),
          let str = String(data: data, encoding: .utf8) else {
        return errorReply(code: ReplyCode.internalError)
    }
    return str
}

// MARK: - Blocking helper for async operations

/// Run an async closure synchronously by blocking with a semaphore. Timeout in seconds.
/// Used for operations that need to wait for a completion handler (permission dialogs, etc.).
func blockingCall(timeout: TimeInterval = 10.0, body: @escaping (@escaping (String) -> Void) -> Void) -> String {
    let semaphore = DispatchSemaphore(value: 0)
    var result = errorReply(code: ReplyCode.timeout)

    body { reply in
        result = reply
        semaphore.signal()
    }

    let deadline = DispatchTime.now() + timeout
    if semaphore.wait(timeout: deadline) == .timedOut {
        return errorReply(code: ReplyCode.timeout)
    }
    return result
}

// MARK: - Blocking helper for main-thread engines

/// An error code of a family, as the contract names it (`bad_base64`, `not_authorized`, …).
/// The `Error` conformance lets it be the `Failure` of a `Result`; `onMainBlocking` turns it
/// back into `{"kind":"error","code":…}`.
struct WireError: Error {
    let code: String
}

/// Run `body` on the main thread and block the caller until `done` answers exactly once.
///
/// Used by engines whose state lives on the main thread (the shared Bluetooth central): the
/// Rust side always enters from `spawn_blocking`, so blocking the caller here is safe, and the
/// engine's delegate callbacks (also delivered on the main queue) can run while it waits.
///
/// The result is a JSON string (already encoded) or an error code, not a `ReplyCode` value: the
/// family owns its own error words (`bad_base64`, `not_authorized`, …). The first answer wins; a
/// second call to `done` is ignored, so an operation that times out cannot be answered twice.
func onMainBlocking(
    timeout: TimeInterval = 15,
    _ body: @escaping (@escaping (Result<String, WireError>) -> Void) -> Void
) -> String {
    let semaphore = DispatchSemaphore(value: 0)
    var finished = false
    var result = errorReply(code: ReplyCode.timeout)
    DispatchQueue.main.async {
        body { outcome in
            guard !finished else { return }
            finished = true
            switch outcome {
            case .success(let json): result = json
            case .failure(let failure): result = errorReply(code: failure.code)
            }
            semaphore.signal()
        }
    }
    _ = semaphore.wait(timeout: .now() + timeout)
    return result
}

/// Run a synchronous closure on the main thread and return its value. For engines whose state
/// is owned by the main thread (the audio recorder): the Rust side enters from `spawn_blocking`,
/// so `sync` never re-enters the main thread, and the guard keeps the helper harmless if it is
/// ever called there.
func onMainSync<T>(_ body: @escaping () -> T) -> T {
    if Thread.isMainThread { return body() }
    var result: T!
    DispatchQueue.main.sync { result = body() }
    return result
}
