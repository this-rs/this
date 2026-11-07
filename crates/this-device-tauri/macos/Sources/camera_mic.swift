// Camera and microphone permissions on macOS (AVFoundation).
//
// Permission only: no frame and no sample is captured here. On macOS the system prompts for
// camera and microphone access just like iOS, through TCC (Transparency, Consent, and Control).
// Requires NSCameraUsageDescription / NSMicrophoneUsageDescription in Info.plist.

import AVFoundation
import Foundation

enum MacCameraMic {

    // MARK: - State queries (no prompt)

    /// The camera permission state, without prompting.
    static func cameraState() -> String {
        guard AVCaptureDevice.default(for: .video) != nil else { return "unsupported" }
        return authName(AVCaptureDevice.authorizationStatus(for: .video))
    }

    /// The microphone permission state, without prompting.
    static func microphoneState() -> String {
        guard AVCaptureDevice.default(for: .audio) != nil else { return "unsupported" }
        return authName(AVCaptureDevice.authorizationStatus(for: .audio))
    }

    /// AVFoundation's authorization status as the contract names it.
    private static func authName(_ status: AVAuthorizationStatus) -> String {
        switch status {
        case .authorized:            return "granted"
        case .denied, .restricted:   return "denied"
        case .notDetermined:         return "prompt"
        @unknown default:            return "prompt"
        }
    }

    // MARK: - Dispatch entry points (called from dispatch.swift)

    /// `captureStatus` — read the state of camera or microphone.
    /// Args: `{"capability": "camera" | "microphone"}`.
    static func status(_ args: [String: Any]) -> String {
        guard let capability = args["capability"] as? String else {
            return statusReply(state: "unsupported")
        }
        switch capability {
        case "camera":     return statusReply(state: cameraState())
        case "microphone": return statusReply(state: microphoneState())
        default:           return statusReply(state: "unsupported")
        }
    }

    /// `captureRequest` — ask the system for camera or microphone permission, then return the
    /// resulting state. If already decided, the system does not prompt again.
    /// Args: `{"capability": "camera" | "microphone"}`.
    static func request(_ args: [String: Any]) -> String {
        guard let capability = args["capability"] as? String else {
            return errorReply(code: ReplyCode.notSupported)
        }
        let mediaType: AVMediaType
        switch capability {
        case "camera":
            guard cameraState() != "unsupported" else {
                return statusReply(state: "unsupported")
            }
            mediaType = .video
        case "microphone":
            guard microphoneState() != "unsupported" else {
                return statusReply(state: "unsupported")
            }
            mediaType = .audio
        default:
            return errorReply(code: ReplyCode.notSupported)
        }

        return blockingCall(timeout: 30) { done in
            AVCaptureDevice.requestAccess(for: mediaType) { _ in
                // Re-read the state after the person's answer.
                let state: String
                switch capability {
                case "camera":     state = cameraState()
                case "microphone": state = microphoneState()
                default:           state = "unsupported"
                }
                done(statusReply(state: state))
            }
        }
    }
}
