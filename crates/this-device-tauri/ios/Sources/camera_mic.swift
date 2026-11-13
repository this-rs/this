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
    guard AVCaptureDevice.default(for: .video) != nil else { return Etat.unsupported.fil }
    return name(AVCaptureDevice.authorizationStatus(for: .video))
  }

  /// The state of the microphone: `unsupported` without a microphone, otherwise the authorization.
  static func microphone() -> String {
    guard AVCaptureDevice.default(for: .audio) != nil else { return Etat.unsupported.fil }
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
    case .authorized: return Etat.granted.fil
    case .denied, .restricted: return Etat.denied.fil
    // Not asked yet: the system asks the first time the capability is requested.
    case .notDetermined: return Etat.prompt.fil
    @unknown default: return Etat.prompt.fil
    }
  }

  /// Asks the system for the permission, then answers the state. When the person has already
  /// answered, the system does not prompt again and answers at once. Without hardware, the
  /// answer is `unsupported` and nothing is asked.
  static func request(_ capability: CaptureCapability, _ invoke: Invoke) {
    guard state(capability) != Etat.unsupported.fil else {
      invoke.resolve(Reply.status(state: Etat.unsupported.fil))
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
      invoke.resolve(Reply.status(state: Etat.unsupported.fil))
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

// ---- La capture ------------------------------------------------------------------------
//
// Une photo, un enregistrement audio borné. Le moteur est `CameraCapture.swift` (photo) et
// `AudioRecorder.swift` (audio), type-checkés seuls contre le SDK iOS ; ici on lit les
// arguments et on répond, comme les autres familles.
//
// Contrat des réponses : `families/camera_mic.rs` (`CaptureReply`). Aucun `reject` : une erreur
// est une réponse, avec le code du contrat.

/// `cameraCapturePhoto` : la plus grande côté en pixels, et la qualité JPEG (le pont a déjà
/// borné les deux).
private struct PhotoArgs: Decodable {
  let maxPx: Int
  let quality: Double

  private enum CodingKeys: String, CodingKey {
    case maxPx = "max_px"
    case quality
  }
}

/// `microphoneStart` : la durée maximale, en millisecondes (bornée par le pont à une minute).
private struct RecordArgs: Decodable {
  let maxMs: Int

  private enum CodingKeys: String, CodingKey {
    case maxMs = "max_ms"
  }
}

/// `{"kind":"recording","max_ms":60000}` — l'enregistrement tourne, il s'arrête seul au bout.
private struct RecordingAnswer: Encodable {
  let kind = "recording"
  let max_ms: Int
}

extension ThisDevicePlugin {
  /// Prendre une photo. Nécessite `captureRequest` accordé : la réponse est `not_authorized`
  /// sinon, jamais une boîte de dialogue ouverte au milieu d'un tour.
  @objc public func cameraCapturePhoto(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(PhotoArgs.self)
    CameraCapture.shared.capture(
      maxPx: args.maxPx,
      quality: args.quality,
      timeoutSeconds: 10
    ) { result in
      switch result {
      case let .success(photo):
        invoke.resolve(photo)
      case let .failure(failure):
        invoke.resolve(Reply.error(code: failure.code))
      }
    }
  }

  /// Démarrer un enregistrement. Rend tout de suite : attendre la fin bloquerait le tour
  /// entier, et le pont coupe à 10 s.
  @objc public func microphoneStart(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(RecordArgs.self)
    switch AudioRecorder.shared.start(maxMs: args.maxMs) {
    case let .success(maxMs):
      invoke.resolve(RecordingAnswer(max_ms: maxMs))
    case let .failure(failure):
      invoke.resolve(Reply.error(code: failure.code))
    }
  }

  /// Arrêter l'enregistrement en cours, ou ramasser celui qui s'est arrêté tout seul au bout.
  @objc public func microphoneStop(_ invoke: Invoke) {
    switch AudioRecorder.shared.stop() {
    case let .success(audio):
      invoke.resolve(audio)
    case let .failure(failure):
      invoke.resolve(Reply.error(code: failure.code))
    }
  }
}

// ---- Filmer ---------------------------------------------------------------------------
//
// La dernière primitive que la caméra n'avait pas. Le fichier, pas les octets : une vidéo de
// cinq secondes fait plusieurs mégaoctets, et le pont garde des faits, pas des charges.
// Elle va dans le dossier temporaire du conteneur et se partage par `shareItem`.

private struct VideoArgs: Decodable {
  let durationMs: Int
  let maxPx: Int

  private enum CodingKeys: String, CodingKey {
    case durationMs = "duration_ms"
    case maxPx = "max_px"
  }
}

private struct VideoOut: Encodable {
  let kind = "video"
  let path: String
  let bytes: Int
  let duration_ms: Int
}

/// Un tournage en vol : la session et le délégué doivent survivre jusqu'à la réponse.
final class VideoCapture: NSObject, AVCaptureFileOutputRecordingDelegate {
  private static var active: [ObjectIdentifier: VideoCapture] = [:]

  private let invoke: Invoke
  private let session = AVCaptureSession()
  private let output = AVCaptureMovieFileOutput()
  private let url: URL
  private var finished = false

  private init(_ invoke: Invoke, url: URL) {
    self.invoke = invoke
    self.url = url
    super.init()
  }

  /// Filme `durationMs` millisecondes, puis répond le chemin du fichier.
  static func run(_ invoke: Invoke, durationMs: Int, maxPx: Int) {
    guard AVCaptureDevice.authorizationStatus(for: .video) == .authorized else {
      invoke.resolve(Reply.error(code: ReplyCode.notAuthorized))
      return
    }
    guard let device = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back)
      ?? AVCaptureDevice.default(for: .video)
    else {
      invoke.resolve(Reply.error(code: "unsupported"))
      return
    }
    let url = FileManager.default.temporaryDirectory
      .appendingPathComponent("atelier-\(UUID().uuidString).mov")
    let capture = VideoCapture(invoke, url: url)
    active[ObjectIdentifier(capture)] = capture
    // Configurer et démarrer une session ne doit pas se faire sur la file principale : le
    // démarrage bloque jusqu'à ce que le matériel réponde.
    DispatchQueue.global(qos: .userInitiated).async {
      do {
        capture.session.beginConfiguration()
        // La qualité se choisit par la session, pas par le fichier : `medium` tient un clip de
        // cinq secondes sous quelques mégaoctets, ce qui est le but.
        capture.session.sessionPreset = maxPx >= 1280 ? .high : .medium
        let input = try AVCaptureDeviceInput(device: device)
        if capture.session.canAddInput(input) { capture.session.addInput(input) }
        if capture.session.canAddOutput(capture.output) {
          capture.session.addOutput(capture.output)
        }
        capture.session.commitConfiguration()
        capture.session.startRunning()
        capture.output.startRecording(to: url, recordingDelegate: capture)
        // L'arrêt : c'est la durée demandée qui le décide, pas un geste.
        DispatchQueue.global().asyncAfter(deadline: .now() + Double(durationMs) / 1000) {
          if capture.output.isRecording { capture.output.stopRecording() }
        }
      } catch {
        capture.finish(Reply.error(code: "unsupported"))
      }
    }
  }

  private func finish(_ reply: Encodable) {
    guard !finished else { return }
    finished = true
    if session.isRunning { session.stopRunning() }
    VideoCapture.active[ObjectIdentifier(self)] = nil
    DispatchQueue.main.async { self.invoke.resolve(reply) }
  }

  func fileOutput(
    _ output: AVCaptureFileOutput,
    didFinishRecordingTo outputFileURL: URL,
    from connections: [AVCaptureConnection],
    error: Error?
  ) {
    if error != nil {
      finish(Reply.error(code: "internal"))
      return
    }
    let size = (try? FileManager.default.attributesOfItem(atPath: outputFileURL.path)[.size]
      as? NSNumber).flatMap { $0 }?.intValue ?? 0
    guard size > 0 else {
      finish(Reply.error(code: "internal"))
      return
    }
    finish(
      VideoOut(
        path: outputFileURL.path,
        bytes: size,
        duration_ms: Int(output.recordedDuration.seconds * 1000)
      ))
  }
}

extension ThisDevicePlugin {
  /// Filme un clip de `duration_ms` millisecondes et rend son chemin.
  @objc public func cameraRecordVideo(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(VideoArgs.self)
    VideoCapture.run(invoke, durationMs: args.durationMs, maxPx: args.maxPx)
  }
}

/// Ancre de l'éditeur de liens — voir `ThisDevicePlugin.famillesAncrees`.
///
/// Un `#selector(Foo.bar)` ne produit pas une référence au SYMBOLE de `bar` : il produit une
/// chaîne. Le dead-strip retire donc l'objet entier, et le pont répond « No command bar found
/// for plugin this-device ». Mesuré le 13/11/2026 sur un iPhone 16 Pro : dix-huit méthodes de
/// familles compilaient, étaient listées dans `famillesLiees`, et étaient **absentes du binaire**.
/// Référencer une CLASSE de ce fichier garde son objet — donc ses méthodes.
final class FamilleCameraMic: NSObject {}
