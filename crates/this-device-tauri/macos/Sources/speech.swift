// Speech recognition on macOS (SFSpeechRecognizer, macOS 10.15+).
//
// Transcribes one audio file, on device, for a bounded duration.
// Privacy: `requiresOnDeviceRecognition = true` — the audio never leaves the machine.
// When the recognizer cannot run on device for the requested locale, the answer is `unsupported`.
//
// Requires NSSpeechRecognitionUsageDescription in Info.plist.

import AVFoundation
import Foundation
import Speech

/// Longest file accepted, in seconds. Kept in step with `SPEECH_SECONDS_MAX` in
/// `src/families/face_speech.rs`.
private let speechSecondsCeiling = 60
/// How long a recognition may run before the call is answered as `timeout`.
private let speechTimeoutSeconds: Double = 120

enum MacSpeech {

    // MARK: - State

    /// The speech permission in the contract's words.
    static func currentState() -> String {
        return authName(SFSpeechRecognizer.authorizationStatus())
    }

    private static func authName(_ status: SFSpeechRecognizerAuthorizationStatus) -> String {
        switch status {
        case .authorized:          return "granted"
        case .denied, .restricted: return "denied"
        case .notDetermined:       return "prompt"
        @unknown default:          return "denied"
        }
    }

    // MARK: - Dispatch entry points

    /// `speechStatus` — the speech permission, read without prompting.
    /// Returns `{"kind":"speech_status","state":"granted"|"denied"|"prompt"}`.
    static func status() -> String {
        return encodeDictJson([
            "kind": "speech_status",
            "state": authName(SFSpeechRecognizer.authorizationStatus()),
        ])
    }

    /// `speechRequest` — ask the person for speech recognition permission.
    /// Returns the resulting state.
    static func request() -> String {
        return blockingCall(timeout: 30) { done in
            SFSpeechRecognizer.requestAuthorization { status in
                done(encodeDictJson([
                    "kind": "speech_status",
                    "state": authName(status),
                ]))
            }
        }
    }

    /// `speechTranscribeFile` — transcribe one audio file, on device.
    /// Args: `{"path": "/...", "locale": "en-US", "max_seconds": 60}`.
    /// Returns `{"kind":"speech_transcript","text":"..."}` or
    /// `{"kind":"speech_refused","code":"unsupported"|"not_authorized"|"too_long"|...}`.
    static func transcribeFile(_ args: [String: Any]) -> String {
        guard let path = args["path"] as? String else {
            return speechRefusal("unreadable")
        }
        let locale = args["locale"] as? String ?? "en-US"
        let maxSecondsRaw = args["max_seconds"] as? Int ?? speechSecondsCeiling
        let maxSeconds = min(max(maxSecondsRaw, 1), speechSecondsCeiling)

        // The file must be inside the app container.
        guard isInsideAppContainer(path) else {
            return speechRefusal("unreadable")
        }

        guard SFSpeechRecognizer.authorizationStatus() == .authorized else {
            return speechRefusal("not_authorized")
        }

        guard
            let recognizer = SFSpeechRecognizer(locale: Locale(identifier: locale)),
            recognizer.supportsOnDeviceRecognition
        else {
            return speechRefusal("unsupported")
        }
        guard recognizer.isAvailable else {
            return speechRefusal("unavailable")
        }

        let url = URL(fileURLWithPath: path)
        guard let file = try? AVAudioFile(forReading: url) else {
            return speechRefusal("unreadable")
        }
        let seconds = Double(file.length) / file.processingFormat.sampleRate
        guard seconds <= Double(maxSeconds) else {
            return speechRefusal("too_long")
        }

        // Run the recognition synchronously with a semaphore.
        let request = SFSpeechURLRecognitionRequest(url: url)
        request.requiresOnDeviceRecognition = true
        request.shouldReportPartialResults = false

        return blockingCall(timeout: speechTimeoutSeconds) { done in
            let task = recognizer.recognitionTask(with: request) { result, error in
                if error != nil {
                    done(speechRefusal("unavailable"))
                    return
                }
                guard let result = result, result.isFinal else { return }
                done(encodeDictJson([
                    "kind": "speech_transcript",
                    "text": result.bestTranscription.formattedString,
                ]))
            }
            // Timeout safety: cancel the task if it runs too long.
            DispatchQueue.global().asyncAfter(deadline: .now() + speechTimeoutSeconds) {
                task.cancel()
            }
        }
    }

    // MARK: - Helpers

    private static func speechRefusal(_ code: String) -> String {
        return encodeDictJson(["kind": "speech_refused", "code": code])
    }

    /// True when the path lies inside the app's own container.
    private static func isInsideAppContainer(_ path: String) -> Bool {
        let resolved = URL(fileURLWithPath: path).resolvingSymlinksInPath().standardizedFileURL.path
        let home = URL(fileURLWithPath: NSHomeDirectory()).resolvingSymlinksInPath().path
        return resolved.hasPrefix(home + "/")
    }
}
