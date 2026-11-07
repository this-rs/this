// Camera and microphone permissions (AVFoundation): the state, read without prompting, and the
// request that asks the system for it.
//
// Permission only: no frame and no sample is captured here. The usage strings
// (NSCameraUsageDescription, NSMicrophoneUsageDescription) are declared by the generated
// Info.plist, from the capability catalogue of this-platform.

import AVFoundation
import Foundation
import Tauri

/// The capabilities of this family, as the Rust side names them.
enum CaptureCapability: String {
  case camera
  case microphone

  var mediaType: AVMediaType {
    switch self {
    case .camera: return .video
    case .microphone: return .audio
    }
  }
}

struct CaptureArgs: Decodable {
  let capability: String
}

/// The permission state of the camera and the microphone, in the contract's words
/// (`PermissionState`).
enum CaptureStatus {
  /// The state of the camera: `unsupported` without a camera, otherwise the authorization.
  static func camera() -> String {
    guard AVCaptureDevice.default(for: .video) != nil else { return "unsupported" }
    return name(AVCaptureDevice.authorizationStatus(for: .video))
  }

  /// The state of the microphone: `unsupported` without a microphone, otherwise the authorization.
  static func microphone() -> String {
    guard AVCaptureDevice.default(for: .audio) != nil else { return "unsupported" }
    return name(AVCaptureDevice.authorizationStatus(for: .audio))
  }

  static func state(_ capability: CaptureCapability) -> String {
    switch capability {
    case .camera: return camera()
    case .microphone: return microphone()
    }
  }

  /// AVFoundation's authorization, as the contract names it.
  static func name(_ authorization: AVAuthorizationStatus) -> String {
    switch authorization {
    case .authorized: return "granted"
    case .denied, .restricted: return "denied"
    // Not asked yet: the system asks the first time the capability is requested.
    case .notDetermined: return "prompt"
    @unknown default: return "prompt"
    }
  }

  /// Asks the system for the permission, then answers the state. When the person has already
  /// answered, the system does not prompt again and answers at once. Without hardware, the
  /// answer is `unsupported` and nothing is asked.
  static func request(_ capability: CaptureCapability, _ invoke: Invoke) {
    guard state(capability) != "unsupported" else {
      invoke.resolve(Reply.status(state: "unsupported"))
      return
    }
    AVCaptureDevice.requestAccess(for: capability.mediaType) { _ in
      // The completion runs on a system queue: the answer is read and sent on the main thread,
      // as the other commands do.
      DispatchQueue.main.async {
        invoke.resolve(Reply.status(state: state(capability)))
      }
    }
  }
}

/// A capability this family does not bridge, asked for in `captureRequest`.
private let notSupportedCode = "not_supported"

extension ThisDevicePlugin {
  /// The permission state of the camera or the microphone, read without prompting the person.
  /// Args: `{"capability": "camera" | "microphone"}`. Answers `Reply.status`.
  @objc public func captureStatus(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(CaptureArgs.self)
    guard let capability = CaptureCapability(rawValue: args.capability) else {
      // Not a capability of this family: the bridge cannot tell, so it says so.
      invoke.resolve(Reply.status(state: "unsupported"))
      return
    }
    invoke.resolve(Reply.status(state: CaptureStatus.state(capability)))
  }

  /// Asks the person for the camera or the microphone permission, if not decided yet, then
  /// answers the state. Shows the system prompt: the caller must have passed the consent gate.
  /// Args: `{"capability": "camera" | "microphone"}`. Answers `Reply.status`, or
  /// `Reply.error` with `not_supported` for another capability.
  @objc public func captureRequest(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(CaptureArgs.self)
    guard let capability = CaptureCapability(rawValue: args.capability) else {
      invoke.resolve(Reply.error(code: notSupportedCode))
      return
    }
    CaptureStatus.request(capability, invoke)
  }
}
