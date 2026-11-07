// Face ID as a confirmation of one action, never as an identity read.
//
// The app asks the system to authenticate the person, with a short reason the system shows.
// The only thing that comes back is a verdict: `granted` or `not_authorized`. Nothing biometric
// is read, stored or sent back: no template, no image, no match score, no identifier. The
// LocalAuthentication API exposes none of them to the app; this file does not look for any.
//
// Policy: `.deviceOwnerAuthenticationWithBiometrics` only (biometry, no passcode fallback).
// Requires NSFaceIDUsageDescription in Info.plist, or the system terminates the app on the
// first evaluation.

import Foundation
import LocalAuthentication
import Tauri

/// Longest reason the system shows, in Unicode scalars. Kept in step with
/// `FACE_ID_REASON_MAX` in `src/families/face_speech.rs`.
private let faceIdReasonMax = 120

private struct FaceIdConfirmArgs: Decodable {
  let reason: String
}

/// The biometry name in the contract's words. A biometry the contract does not know (optic ID)
/// is reported as `none`: the bridge offers no Face ID or Touch ID there, so it says so.
private func faceIdBiometryName(_ type: LABiometryType) -> String {
  switch type {
  case .faceID: return "face_id"
  case .touchID: return "touch_id"
  default: return "none"
  }
}

/// The reason is non-empty after trimming, at most `faceIdReasonMax` scalars, and has no
/// control character. Same rule as `check_reason` on the Rust side.
private func isValidFaceIdReason(_ reason: String) -> Bool {
  let trimmed = reason.trimmingCharacters(in: .whitespacesAndNewlines)
  guard !trimmed.isEmpty, trimmed.unicodeScalars.count <= faceIdReasonMax else {
    return false
  }
  return !trimmed.unicodeScalars.contains { CharacterSet.controlCharacters.contains($0) }
}

extension ThisDevicePlugin {
  /// The biometry available, read without prompting the person: `face_id`, `touch_id`, `none`.
  /// Answers `{"kind":"face_id_biometry","biometry":...}`.
  @objc public func faceIdStatus(_ invoke: Invoke) {
    let context = LAContext()
    var error: NSError?
    guard context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error) else {
      // Not enrolled, locked out, or no biometry: no biometry is usable right now.
      let payload: JsonObject = ["kind": "face_id_biometry", "biometry": "none"]
      invoke.resolve(payload)
      return
    }
    let payload: JsonObject = [
      "kind": "face_id_biometry",
      "biometry": faceIdBiometryName(context.biometryType),
    ]
    invoke.resolve(payload)
  }

  /// Asks the person to confirm one action with biometry. The argument `reason` is the motif
  /// the system shows; it is checked here as well as by the Rust caller. Answers
  /// `{"kind":"face_id","verdict":"granted"}` or `{"kind":"face_id","verdict":"not_authorized"}`.
  /// Any failure to evaluate (cancel, failed match, lockout, no biometry) is `not_authorized`.
  /// A malformed reason is a rejected call, not a verdict.
  @objc public func faceIdConfirm(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(FaceIdConfirmArgs.self)
    guard isValidFaceIdReason(args.reason) else {
      invoke.reject("invalid_reason")
      return
    }
    let context = LAContext()
    var error: NSError?
    guard context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error) else {
      let payload: JsonObject = ["kind": "face_id", "verdict": "not_authorized"]
      invoke.resolve(payload)
      return
    }
    context.evaluatePolicy(
      .deviceOwnerAuthenticationWithBiometrics, localizedReason: args.reason
    ) { success, _ in
      // The error is deliberately dropped: the verdict is a boolean, and no reason for a
      // refusal is passed back to the app.
      DispatchQueue.main.async {
        // Keeps the context alive until the system has answered.
        withExtendedLifetime(context) {
          let verdict = success ? "granted" : "not_authorized"
          let payload: JsonObject = ["kind": "face_id", "verdict": verdict]
          invoke.resolve(payload)
        }
      }
    }
  }
}
