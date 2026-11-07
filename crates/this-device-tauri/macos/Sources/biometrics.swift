// Biometrics (Touch ID) on macOS via LocalAuthentication.
//
// On macOS the biometry is Touch ID (MacBook Pro/Air with Touch Bar or Apple Silicon).
// The wire names keep "face_id" for compatibility with the iOS contract: the Rust side uses
// `Capability::FaceId` regardless of the actual biometry type.
//
// Policy: `.deviceOwnerAuthenticationWithBiometrics` only (biometry, no password fallback).
// Nothing biometric is read, stored or sent back — only a boolean verdict.

import Foundation
import LocalAuthentication

/// Longest reason the system shows, in Unicode scalars. Kept in step with
/// `FACE_ID_REASON_MAX` in `src/families/face_speech.rs`.
private let reasonMaxScalars = 120

enum MacBiometrics {

    // MARK: - State

    /// The biometry type available on this Mac: `touch_id`, `face_id` (future), or `none`.
    static func currentState() -> String {
        let context = LAContext()
        var error: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error) else {
            return "unsupported"
        }
        // If biometry is available, report as granted (the capability is present).
        // The actual "granted/denied" is per-evaluation, not per-permission.
        return "granted"
    }

    private static func biometryName(_ type: LABiometryType) -> String {
        switch type {
        case .touchID: return "touch_id"
        case .faceID:  return "face_id"
        default:       return "none"
        }
    }

    // MARK: - Dispatch entry points

    /// `faceIdStatus` — the biometry available, without prompting.
    /// Returns `{"kind":"face_id_biometry","biometry":"touch_id"|"face_id"|"none"}`.
    static func status() -> String {
        let context = LAContext()
        var error: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error) else {
            return encodeDictJson(["kind": "face_id_biometry", "biometry": "none"])
        }
        return encodeDictJson([
            "kind": "face_id_biometry",
            "biometry": biometryName(context.biometryType),
        ])
    }

    /// `faceIdConfirm` — ask the person to authenticate with biometry.
    /// Args: `{"reason": "short text shown by the system"}`.
    /// Returns `{"kind":"face_id","verdict":"granted"|"not_authorized"}`.
    static func confirm(_ args: [String: Any]) -> String {
        guard let reason = args["reason"] as? String, isValidReason(reason) else {
            return errorReply(code: ReplyCode.internalError)
        }

        let context = LAContext()
        var error: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error) else {
            return encodeDictJson(["kind": "face_id", "verdict": "not_authorized"])
        }

        return blockingCall(timeout: 60) { done in
            context.evaluatePolicy(
                .deviceOwnerAuthenticationWithBiometrics,
                localizedReason: reason
            ) { success, _ in
                // Keep the context alive until the system has answered.
                withExtendedLifetime(context) {
                    let verdict = success ? "granted" : "not_authorized"
                    done(encodeDictJson(["kind": "face_id", "verdict": verdict]))
                }
            }
        }
    }

    /// Validate the reason string: non-empty after trimming, bounded length, no control chars.
    private static func isValidReason(_ reason: String) -> Bool {
        let trimmed = reason.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed.unicodeScalars.count <= reasonMaxScalars else {
            return false
        }
        return !trimmed.unicodeScalars.contains { CharacterSet.controlCharacters.contains($0) }
    }
}
