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
  /// Langue de la reconnaissance. Facultative : quand l'appelant ne la nomme pas, c'est celle de
  /// l'appareil — et lui seul la connaît. La rendre obligatoire obligeait un appelant qui ne
  /// connaît pas la signature à demander la langue par un autre appel, puis à retenter ; ou à
  /// deviner « fr-FR », ce qui transcrit en français un mémo dicté en anglais.
  let locale: String?
  /// Plafond d'écoute. Facultatif : le pont le remplit (`SPEECH_SECONDS_MAX`) ; ce repli ne
  /// sert qu'à un appel qui contournerait le contrat.
  let maxSeconds: Int?

  private enum CodingKeys: String, CodingKey {
    case path, locale
    case maxSeconds = "max_seconds"
  }
}

/// La langue de reconnaissance : celle qu'on demande, sinon celle de l'appareil.
fileprivate func speechLocale(_ asked: String?) -> Locale {
  guard let asked = asked?.trimmingCharacters(in: .whitespaces), !asked.isEmpty else {
    return Locale.current
  }
  return Locale(identifier: asked)
}

/// The speech permission in the contract's words (`PermissionState`).
private func speechStateName(_ status: SFSpeechRecognizerAuthorizationStatus) -> String {
  switch status {
  case .authorized: return Etat.granted.fil
  case .denied, .restricted: return Etat.denied.fil
  case .notDetermined: return Etat.prompt.fil
  // A state the system adds later is not known to be allowed: fail closed.
  @unknown default: return Etat.denied.fil
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
    let maxSeconds = min(max(args.maxSeconds ?? speechSecondsCeiling, 1), speechSecondsCeiling)

    guard isInsideAppContainer(args.path) else {
      invoke.resolve(speechRefusal("unreadable"))
      return
    }
    guard SFSpeechRecognizer.authorizationStatus() == .authorized else {
      invoke.resolve(speechRefusal("not_authorized"))
      return
    }
    guard
      let recognizer = SFSpeechRecognizer(locale: speechLocale(args.locale)),
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

/// Ancre de l'éditeur de liens — voir `ThisDevicePlugin.famillesAncrees`.
///
/// Un `#selector(Foo.bar)` ne produit pas une référence au SYMBOLE de `bar` : il produit une
/// chaîne. Le dead-strip retire donc l'objet entier, et le pont répond « No command bar found
/// for plugin this-device ». Mesuré le 13/11/2026 sur un iPhone 16 Pro : dix-huit méthodes de
/// familles compilaient, étaient listées dans `famillesLiees`, et étaient **absentes du binaire**.
/// Référencer une CLASSE de ce fichier garde son objet — donc ses méthodes.
final class FamilleSpeech: NSObject {}
