// NDEF read, on demand (CoreNFC). Reading only: no write.
//
// Two commands, both on the plugin class (see ThisDevicePlugin.swift):
// - `nfcStatus`: whether this device has an NFC reader, read without opening a session;
// - `nfcReadOnce`: one NDEF session, closed on every path (a tag read, a timeout, a cancel,
//   an error). It answers with the records of the first NDEF message.
//
// Requirements, not added here:
// - NFCReaderUsageDescription in Info.plist (in the capability catalogue);
// - the entitlement com.apple.developer.nfc.readersession.formats containing NDEF in the app
//   bundle. Without it, the session cannot begin.

import CoreNFC
import Foundation
import Tauri

struct NfcReadArgs: Decodable {
  let timeoutMs: Int?

  private enum CodingKeys: String, CodingKey {
    case timeoutMs = "timeout_ms"
  }
}

/// The NFC answers. Own type, not `Reply`: the NFC contract is its own shape on the Rust side
/// (`families/nfc.rs`). Every answer is one JSON object with a `kind`.
enum NfcReply: Encodable {
  case status(state: String)
  case ndef(records: [NdefRecordOut])
  case error(code: String)

  private enum Key: String, CodingKey {
    case kind, state, records, code
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    switch self {
    case let .status(state):
      try c.encode("status", forKey: .kind)
      try c.encode(state, forKey: .state)
    case let .ndef(records):
      try c.encode("ndef", forKey: .kind)
      try c.encode(records, forKey: .records)
    case let .error(code):
      try c.encode("error", forKey: .kind)
      try c.encode(code, forKey: .code)
    }
  }
}

/// One NDEF record. Optional fields are written as `null`, never omitted.
struct NdefRecordOut: Encodable {
  let tnf: String
  let type: String?
  let payloadText: String?
  let payloadBase64: String

  private enum Key: String, CodingKey {
    case tnf, type, payload_text, payload_base64
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    try c.encode(tnf, forKey: .tnf)
    // `encode` (not `encodeIfPresent`) writes `null` for an absent value.
    try c.encode(type, forKey: .type)
    try c.encode(payloadText, forKey: .payload_text)
    try c.encode(payloadBase64, forKey: .payload_base64)
  }
}

extension ThisDevicePlugin {
  /// Whether this device has an NFC reader. No session is opened and nothing is prompted.
  /// `granted` when a reader exists: NFC has no permission to query before a session, the
  /// system asks when the first session begins.
  @objc public func nfcStatus(_ invoke: Invoke) {
    invoke.resolve(
      NfcReply.status(state: NFCNDEFReaderSession.readingAvailable ? "granted" : "unsupported"))
  }

  /// One NDEF read, bounded by `timeout_ms` (clamped to the native range below).
  @objc public func nfcReadOnce(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(NfcReadArgs.self)
    NfcReadSession.read(invoke, timeoutMs: NfcReadSession.clampedTimeoutMs(args.timeoutMs))
  }
}

/// One NDEF session. It keeps itself alive until it answers: the caller only holds the `Invoke`.
final class NfcReadSession: NSObject, NFCNDEFReaderSessionDelegate {
  // Mirrors `NFC_READ_MS_*` in families/nfc.rs. The Rust side clamps first; this is the floor.
  static let timeoutMsMin = 1_000
  static let timeoutMsMax = 15_000
  static let timeoutMsDefault = 15_000

  private static var active: [ObjectIdentifier: NfcReadSession] = [:]

  private let invoke: Invoke
  private var session: NFCNDEFReaderSession?
  private var finished = false

  private init(_ invoke: Invoke) {
    self.invoke = invoke
    super.init()
  }

  static func clampedTimeoutMs(_ requested: Int?) -> Int {
    min(max(requested ?? timeoutMsDefault, timeoutMsMin), timeoutMsMax)
  }

  /// Called on the main thread with a timeout already in range.
  static func read(_ invoke: Invoke, timeoutMs: Int) {
    guard NFCNDEFReaderSession.readingAvailable else {
      invoke.resolve(NfcReply.error(code: "unavailable"))
      return
    }
    let pending = NfcReadSession(invoke)
    active[ObjectIdentifier(pending)] = pending
    // Delegate callbacks on the main queue. `invalidateAfterFirstRead` ends the session after
    // the first tag, so a second tag in range is not read by accident.
    let reader = NFCNDEFReaderSession(delegate: pending, queue: nil, invalidateAfterFirstRead: true)
    reader.alertMessage = "Approchez un tag NFC."
    pending.session = reader
    reader.begin()
    DispatchQueue.main.asyncAfter(deadline: .now() + Double(timeoutMs) / 1000) { [weak pending] in
      pending?.finish(NfcReply.error(code: "timeout"))
    }
  }

  // MARK: NFCNDEFReaderSessionDelegate

  func readerSession(_ session: NFCNDEFReaderSession, didDetectNDEFs messages: [NFCNDEFMessage]) {
    let records = messages.flatMap { $0.records }.map(NfcReadSession.describe)
    finish(NfcReply.ndef(records: records))
  }

  func readerSession(_ session: NFCNDEFReaderSession, didInvalidateWithError error: Error) {
    // Also called after a successful read and after our own `invalidate`: `finish` ignores those.
    guard let nfcError = error as? NFCReaderError else {
      finish(NfcReply.error(code: "internal"))
      return
    }
    switch nfcError.code {
    case .readerSessionInvalidationErrorUserCanceled:
      finish(NfcReply.error(code: "cancelled"))
    case .readerSessionInvalidationErrorSystemIsBusy:
      finish(NfcReply.error(code: "unavailable"))
    default:
      finish(NfcReply.error(code: "internal"))
    }
  }

  // MARK: Private

  private func finish(_ reply: NfcReply) {
    guard !finished else { return }
    finished = true
    // Closes the session on every path. A no-op when the system already invalidated it.
    session?.invalidate()
    invoke.resolve(reply)
    NfcReadSession.active[ObjectIdentifier(self)] = nil
  }

  /// One record as the Rust side reads it. Text and URI payloads are decoded; the raw payload
  /// is always sent in base64.
  static func describe(_ record: NFCNDEFPayload) -> NdefRecordOut {
    var text: String? = nil
    if record.typeNameFormat == .nfcWellKnown {
      if let t = record.wellKnownTypeTextPayload().0 {
        text = t
      } else if let u = record.wellKnownTypeURIPayload() {
        text = u.absoluteString
      }
    }
    return NdefRecordOut(
      tnf: typeNameFormatName(record.typeNameFormat),
      type: String(data: record.type, encoding: .utf8),
      payloadText: text,
      payloadBase64: record.payload.base64EncodedString())
  }

  static func typeNameFormatName(_ format: NFCTypeNameFormat) -> String {
    switch format {
    case .nfcWellKnown: return "well_known"
    case .media: return "media"
    case .absoluteURI: return "absolute_uri"
    case .nfcExternal: return "external"
    case .empty: return "empty"
    case .unchanged: return "unchanged"
    case .unknown: return "unknown"
    @unknown default: return "unknown"
    }
  }
}
