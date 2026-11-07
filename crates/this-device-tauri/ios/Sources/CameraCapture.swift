// One photo, taken with the back camera (AVFoundation, AVCapturePhotoOutput).
//
// NO TAURI IMPORT, on purpose: this file is type-checked on its own with the iOS SDK
// (`xcrun --sdk iphoneos swiftc -typecheck -target arm64-apple-ios17.0 CameraCapture.swift
// AudioRecorder.swift`). The selectors that parse arguments and resolve the invocation live in
// `camera_mic.swift`.
//
// ## Why the session is kept
//
// Configuring an AVCaptureSession and letting it warm up costs about a second. An agent that
// takes two photos in a row should not pay it twice, so the session stays alive between calls
// and is stopped (not destroyed) once the answer is sent.
//
// ## Why the image always goes through UIImage
//
// `AVCapturePhoto.fileDataRepresentation()` gives a full-resolution JPEG: 4 to 8 MB on a modern
// phone, which is 6 to 11 MB of base64 crossing the webview bridge for a photo that will be
// shown in a chat bubble. Every photo is therefore re-encoded with `max_px` as the largest side
// and the requested quality — the bound is the point, not an optimisation.

import AVFoundation
import Foundation
import UIKit

/// One photo, as the Rust side reads it (`families/camera_mic.rs`, `Photo`).
struct CapturedPhoto: Encodable {
  /// The wire tag: the Rust side reads `{"kind":"photo",…}`.
  let kind = "photo"
  let data_base64: String
  let width: Int
  let height: Int
  let bytes: Int
}

/// A refusal or a failure, in the contract's words (`CaptureError`).
enum CaptureFailure: Error {
  case code(String)

  var code: String {
    switch self {
    case let .code(value): return value
    }
  }
}

final class CameraCapture: NSObject {
  static let shared = CameraCapture()

  /// The session and the delegate callbacks live on this queue: `startRunning` must never run
  /// on the main thread, it blocks until the hardware answers.
  private let queue = DispatchQueue(label: "dev.thisrs.atelier.camera")
  private var session: AVCaptureSession?
  private var output: AVCapturePhotoOutput?
  private var pending: ((Result<CapturedPhoto, CaptureFailure>) -> Void)?
  private var answered = false
  private var maxPx = 1280
  private var quality = 0.8

  /// Take one photo. The completion runs on the main thread, once.
  func capture(
    maxPx: Int,
    quality: Double,
    timeoutSeconds: Double,
    done: @escaping (Result<CapturedPhoto, CaptureFailure>) -> Void
  ) {
    // Read without prompting: the caller has already been through `captureRequest`, which is
    // the only method allowed to show the system dialog.
    guard AVCaptureDevice.authorizationStatus(for: .video) == .authorized else {
      done(.failure(.code("not_authorized")))
      return
    }
    let device =
      AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back)
      ?? AVCaptureDevice.default(for: .video)
    guard let device = device else {
      done(.failure(.code("unsupported")))
      return
    }
    guard pending == nil else {
      // Two captures at once would interleave their delegate callbacks on the same output.
      done(.failure(.code("unavailable")))
      return
    }

    self.maxPx = maxPx
    self.quality = quality
    pending = done
    answered = false

    queue.async { [weak self] in
      guard let self = self else { return }
      do {
        let session = try self.configured(device: device)
        if !session.isRunning { session.startRunning() }
        guard let output = self.output else {
          self.finish(.failure(.code("internal")))
          return
        }
        let settings = AVCapturePhotoSettings(format: [AVVideoCodecKey: AVVideoCodecType.jpeg])
        // `.balanced` rather than `.quality`: the photo is re-encoded anyway, and a quality
        // capture takes hundreds of milliseconds more for pixels nobody keeps.
        settings.photoQualityPrioritization = .balanced
        output.capturePhoto(with: settings, delegate: self)
      } catch {
        self.finish(.failure(.code("unsupported")))
      }
    }

    DispatchQueue.main.asyncAfter(deadline: .now() + timeoutSeconds) { [weak self] in
      self?.finish(.failure(.code("timeout")))
    }
  }

  /// The session, configured once. Called on `queue`.
  private func configured(device: AVCaptureDevice) throws -> AVCaptureSession {
    if let session = session, !session.inputs.isEmpty {
      return session
    }
    let session = AVCaptureSession()
    session.beginConfiguration()
    session.sessionPreset = .photo
    let input = try AVCaptureDeviceInput(device: device)
    if session.canAddInput(input) {
      session.addInput(input)
    } else {
      session.commitConfiguration()
      throw CaptureFailure.code("unsupported")
    }
    let output = AVCapturePhotoOutput()
    if session.canAddOutput(output) {
      session.addOutput(output)
    } else {
      session.commitConfiguration()
      throw CaptureFailure.code("unsupported")
    }
    session.commitConfiguration()
    self.session = session
    self.output = output
    return session
  }

  /// Answer exactly once, whatever gets there first: the photo, the timeout, or an error.
  private func finish(_ result: Result<CapturedPhoto, CaptureFailure>) {
    guard !answered else { return }
    answered = true
    let done = pending
    pending = nil
    let session = self.session
    queue.async {
      if let session = session, session.isRunning { session.stopRunning() }
    }
    DispatchQueue.main.async {
      done?(result)
    }
  }

  /// Re-encode to the bounded size. `UIImage` is the only resampling available without pulling
  /// a graphics library into the bridge.
  private func bounded(_ data: Data) -> CapturedPhoto? {
    guard let image = UIImage(data: data) else { return nil }
    let largest = max(image.size.width, image.size.height)
    let scale = largest > CGFloat(maxPx) ? CGFloat(maxPx) / largest : 1
    var rendered = image
    if scale < 1 {
      let size = CGSize(
        width: (image.size.width * scale).rounded(),
        height: (image.size.height * scale).rounded()
      )
      let format = UIGraphicsImageRendererFormat.default()
      format.scale = 1
      rendered = UIGraphicsImageRenderer(size: size, format: format).image { _ in
        image.draw(in: CGRect(origin: .zero, size: size))
      }
    }
    guard let jpeg = rendered.jpegData(compressionQuality: quality) else { return nil }
    return CapturedPhoto(
      data_base64: jpeg.base64EncodedString(),
      width: Int(rendered.size.width.rounded()),
      height: Int(rendered.size.height.rounded()),
      bytes: jpeg.count
    )
  }
}

extension CameraCapture: AVCapturePhotoCaptureDelegate {
  func photoOutput(
    _ output: AVCapturePhotoOutput,
    didFinishProcessingPhoto photo: AVCapturePhoto,
    error: Error?
  ) {
    if error != nil {
      finish(.failure(.code("internal")))
      return
    }
    guard let data = photo.fileDataRepresentation(), let bounded = bounded(data) else {
      finish(.failure(.code("internal")))
      return
    }
    finish(.success(bounded))
  }
}
