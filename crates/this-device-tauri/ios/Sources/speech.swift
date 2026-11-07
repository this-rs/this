// Speech recognition of one audio file the app already holds (SFSpeechRecognizer).
//
// Not the microphone: live capture is another family and is not here.
//
// Privacy: the transcription is forced on device (`requiresOnDeviceRecognition`). The audio does
// not leave the phone. When the recognizer cannot run on device for the language, the answer is
// `unsupported`; there is no fallback to Apple's servers.
//
// Requires NSSpeechRecognitionUsageDescription in Info.plist, or the system terminates the app
// on the first authorization request.

import AVFoundation
import Foundation
import Speech
import Tauri

/// Longest file accepted, in seconds. Kept in step with `SPEECH_SECONDS_MAX` in
/// `src/families/face_speech.rs`.
private let speechSecondsCeiling = 60
/// How long a recognition may run before the call is answered as `timeout`.
private let speechTimeoutSeconds: Double = 120

fileprivate struct SpeechTranscribeArgs: Decodable {
  let path: String
  let locale: String
  let maxSeconds: Int

  private enum CodingKeys: String, CodingKey {
    case path, locale
    case maxSeconds = "max_seconds"
  }
}

/// The speech permission in the contract's words (`PermissionState`).
private func speechStateName(_ status: SFSpeechRecognizerAuthorizationStatus) -> String {
  switch status {
  case .authorized: return "granted"
  case .denied, .restricted: return "denied"
  case .notDetermined: return "prompt"
  // A state the system adds later is not known to be allowed: fail closed.
  @unknown default: return "denied"
  }
}

private func speechStatusPayload(_ status: SFSpeechRecognizerAuthorizationStatus) -> JsonObject {
  return ["kind": "speech_status", "state": speechStateName(status)]
}

private func speechRefusal(_ code: String) -> JsonObject {
  return ["kind": "speech_refused", "code": code]
}

/// True when the path, once symlinks and `..` are resolved, lies inside the app's own container.
/// The webview never gets to have arbitrary files of the device transcribed.
private func isInsideAppContainer(_ path: String) -> Bool {
  let resolved = URL(fileURLWithPath: path).resolvingSymlinksInPath().standardizedFileURL.path
  let home = URL(fileURLWithPath: NSHomeDirectory()).resolvingSymlinksInPath().path
  return resolved.hasPrefix(home + "/")
}

extension ThisDevicePlugin {
  /// The speech permission, read without prompting the person.
  /// Answers `{"kind":"speech_status","state":"granted"|"denied"|"prompt"}`.
  @objc public func speechStatus(_ invoke: Invoke) {
    invoke.resolve(speechStatusPayload(SFSpeechRecognizer.authorizationStatus()))
  }

  /// Asks the person for the speech permission, then answers with the state the system chose.
  @objc public func speechRequest(_ invoke: Invoke) {
    SFSpeechRecognizer.requestAuthorization { status in
      DispatchQueue.main.async {
        invoke.resolve(speechStatusPayload(status))
      }
    }
  }

  /// Transcribes one audio file, on device, for a bounded duration.
  /// Answers `{"kind":"speech_transcript","text":...}` or `{"kind":"speech_refused","code":...}`
  /// with a code among `unsupported`, `not_authorized`, `too_long`, `unreadable`, `unavailable`,
  /// `timeout`.
  @objc public func speechTranscribeFile(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(SpeechTranscribeArgs.self)
    SpeechTranscription.run(invoke, args: args)
  }
}

/// One transcription. The session keeps itself alive until it answers: the caller only holds the
/// `Invoke`. Called on the main thread.
final class SpeechTranscription {
  private static var active: [ObjectIdentifier: SpeechTranscription] = [:]

  private let invoke: Invoke
  private var task: SFSpeechRecognitionTask?
  private var finished = false

  private init(_ invoke: Invoke) {
    self.invoke = invoke
  }

  fileprivate static func run(_ invoke: Invoke, args: SpeechTranscribeArgs) {
    // The Rust side has already checked the bounds; they are repeated here so the native side
    // never trusts the caller alone.
    let maxSeconds = min(max(args.maxSeconds, 1), speechSecondsCeiling)

    guard isInsideAppContainer(args.path) else {
      invoke.resolve(speechRefusal("unreadable"))
      return
    }
    guard SFSpeechRecognizer.authorizationStatus() == .authorized else {
      invoke.resolve(speechRefusal("not_authorized"))
      return
    }
    guard
      let recognizer = SFSpeechRecognizer(locale: Locale(identifier: args.locale)),
      recognizer.supportsOnDeviceRecognition
    else {
      invoke.resolve(speechRefusal("unsupported"))
      return
    }
    guard recognizer.isAvailable else {
      invoke.resolve(speechRefusal("unavailable"))
      return
    }
    let url = URL(fileURLWithPath: args.path)
    guard let file = try? AVAudioFile(forReading: url) else {
      invoke.resolve(speechRefusal("unreadable"))
      return
    }
    let seconds = Double(file.length) / file.processingFormat.sampleRate
    guard seconds <= Double(maxSeconds) else {
      invoke.resolve(speechRefusal("too_long"))
      return
    }

    let session = SpeechTranscription(invoke)
    active[ObjectIdentifier(session)] = session

    let request = SFSpeechURLRecognitionRequest(url: url)
    request.requiresOnDeviceRecognition = true
    request.shouldReportPartialResults = false
    session.task = recognizer.recognitionTask(with: request) { [weak session] result, error in
      guard let session = session else { return }
      if error != nil {
        session.finish(speechRefusal("unavailable"))
        return
      }
      guard let result = result, result.isFinal else { return }
      session.finish([
        "kind": "speech_transcript",
        "text": result.bestTranscription.formattedString,
      ])
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + speechTimeoutSeconds) { [weak session] in
      session?.finish(speechRefusal("timeout"))
    }
  }

  private func finish(_ payload: JsonObject) {
    guard !finished else { return }
    finished = true
    task?.cancel()
    invoke.resolve(payload)
    SpeechTranscription.active[ObjectIdentifier(self)] = nil
  }
}
