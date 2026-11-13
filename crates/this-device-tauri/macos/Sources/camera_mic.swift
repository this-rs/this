// Camera and microphone permissions on macOS (AVFoundation).
//
// Permission only: no frame and no sample is captured here. On macOS the system prompts for
// camera and microphone access just like iOS, through TCC (Transparency, Consent, and Control).
// Requires NSCameraUsageDescription / NSMicrophoneUsageDescription in Info.plist.

import AVFoundation
import AppKit
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

// MARK: - The capture itself (a port of ios/Sources/camera_mic.swift, CameraCapture.swift and
// AudioRecorder.swift, adapted to the macOS APIs: `AVCaptureDevice.default(for: .video)` instead
// of the iOS `.builtInWideAngleCamera`, `NSBitmapImageRep` instead of `UIImage`, and no
// `AVAudioSession`, which does not exist on macOS.)
//
// Contrat des réponses : `families/camera_mic.rs` (`CaptureReply`). Aucun `reject` : une erreur
// est une réponse, avec le code du contrat.

private struct PhotoAnswer: Encodable {
    let kind = "photo"
    let data_base64: String
    let width: Int
    let height: Int
    let bytes: Int
}

private struct VideoAnswer: Encodable {
    let kind = "video"
    let path: String
    let bytes: Int
    let duration_ms: Int
}

private struct AudioAnswer: Encodable {
    let kind = "audio"
    let path: String
    let bytes: UInt64
    let duration_ms: UInt64
}

private struct RecordingAnswer: Encodable {
    let kind = "recording"
    let max_ms: Int
}

/// One photo, re-encoded to the bounded size. `max_px` is the largest side, as on iOS.
private final class MacCameraCapture: NSObject, AVCapturePhotoCaptureDelegate {
    static let shared = MacCameraCapture()

    /// The session and the delegate callbacks live on this queue: `startRunning` must never run
    /// on the main thread, it blocks until the hardware answers.
    private let queue = DispatchQueue(label: "dev.thisrs.device.camera")
    private var session: AVCaptureSession?
    private var output: AVCapturePhotoOutput?
    private var pending: ((Result<PhotoAnswer, WireError>) -> Void)?
    private var answered = false
    private var maxPx = 1280
    private var quality = 0.8

    func capture(
        maxPx: Int,
        quality: Double,
        done: @escaping (Result<PhotoAnswer, WireError>) -> Void
    ) {
        // Read without prompting: the caller has already been through `captureRequest`, the only
        // method allowed to show the system dialog.
        guard AVCaptureDevice.authorizationStatus(for: .video) == .authorized else {
            done(.failure(WireError(code: "not_authorized")))
            return
        }
        guard let device = AVCaptureDevice.default(for: .video) else {
            done(.failure(WireError(code: "unsupported")))
            return
        }

        queue.async { [weak self] in
            guard let self = self else { return }
            if self.pending != nil {
                done(.failure(WireError(code: "unavailable")))
                return
            }
            self.maxPx = maxPx
            self.quality = quality
            self.pending = done
            self.answered = false
            do {
                let session = try self.configured(device: device)
                if !session.isRunning { session.startRunning() }
                guard let output = self.output else {
                    self.finish(.failure(WireError(code: "internal")))
                    return
                }
                let settings = AVCapturePhotoSettings(format: [AVVideoCodecKey: AVVideoCodecType.jpeg])
                output.capturePhoto(with: settings, delegate: self)
            } catch {
                self.finish(.failure(WireError(code: "unsupported")))
            }
        }

        DispatchQueue.main.asyncAfter(deadline: .now() + 10) { [weak self] in
            self?.finish(.failure(WireError(code: "timeout")))
        }
    }

    private func configured(device: AVCaptureDevice) throws -> AVCaptureSession {
        if let session = session, !session.inputs.isEmpty { return session }
        let session = AVCaptureSession()
        session.beginConfiguration()
        session.sessionPreset = .photo
        let input = try AVCaptureDeviceInput(device: device)
        if session.canAddInput(input) {
            session.addInput(input)
        } else {
            session.commitConfiguration()
            throw WireError(code: "unsupported")
        }
        let output = AVCapturePhotoOutput()
        if session.canAddOutput(output) {
            session.addOutput(output)
        } else {
            session.commitConfiguration()
            throw WireError(code: "unsupported")
        }
        session.commitConfiguration()
        self.session = session
        self.output = output
        return session
    }

    /// Answer exactly once, whatever gets there first: the photo, the timeout, or an error.
    private func finish(_ result: Result<PhotoAnswer, WireError>) {
        queue.async { [weak self] in
            guard let self = self, !self.answered else { return }
            self.answered = true
            let done = self.pending
            self.pending = nil
            let session = self.session
            if let session = session, session.isRunning { session.stopRunning() }
            DispatchQueue.main.async { done?(result) }
        }
    }

    private func bounded(_ data: Data) -> PhotoAnswer? {
        guard let (jpeg, width, height) = Self.resample(data, maxPx: maxPx, quality: quality) else {
            return nil
        }
        return PhotoAnswer(
            data_base64: jpeg.base64EncodedString(),
            width: width,
            height: height,
            bytes: jpeg.count
        )
    }

    /// Re-encode to the bounded size. `NSBitmapImageRep` is the AppKit equivalent of the iOS
    /// `UIImage` path: no graphics library beyond what the app already links.
    private static func resample(_ jpeg: Data, maxPx: Int, quality: Double) -> (Data, Int, Int)? {
        guard let source = CGImageSourceCreateWithData(jpeg as CFData, nil),
              let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else { return nil }
        let width = image.width
        let height = image.height
        let largest = max(width, height)
        let scale = largest > maxPx ? Double(maxPx) / Double(largest) : 1.0
        let targetWidth = max(1, Int((Double(width) * scale).rounded()))
        let targetHeight = max(1, Int((Double(height) * scale).rounded()))

        let resized: CGImage
        if scale >= 1.0 {
            resized = image
        } else {
            guard let ctx = CGContext(
                data: nil, width: targetWidth, height: targetHeight,
                bitsPerComponent: 8, bytesPerRow: 0,
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue
            ) else { return nil }
            ctx.interpolationQuality = .high
            ctx.draw(image, in: CGRect(x: 0, y: 0, width: targetWidth, height: targetHeight))
            guard let drawn = ctx.makeImage() else { return nil }
            resized = drawn
        }

        let rep = NSBitmapImageRep(cgImage: resized)
        guard let data = rep.representation(using: .jpeg, properties: [.compressionFactor: quality]) else {
            return nil
        }
        return (data, resized.width, resized.height)
    }

    func photoOutput(_ output: AVCapturePhotoOutput, didFinishProcessingPhoto photo: AVCapturePhoto, error: Error?) {
        if error != nil {
            finish(.failure(WireError(code: "internal")))
            return
        }
        guard let data = photo.fileDataRepresentation(), let bounded = bounded(data) else {
            finish(.failure(WireError(code: "internal")))
            return
        }
        finish(.success(bounded))
    }
}

/// A bounded video clip, written to the app's temporary directory (the file, not the bytes).
private final class MacVideoCapture: NSObject, AVCaptureFileOutputRecordingDelegate {
    private static var active: [ObjectIdentifier: MacVideoCapture] = [:]

    private let session = AVCaptureSession()
    private let output = AVCaptureMovieFileOutput()
    private let url: URL
    private var done: ((Result<VideoAnswer, WireError>) -> Void)?
    private var finished = false

    private init(_ done: @escaping (Result<VideoAnswer, WireError>) -> Void, url: URL) {
        self.done = done
        self.url = url
        super.init()
    }

    static func run(
        _ done: @escaping (Result<VideoAnswer, WireError>) -> Void,
        durationMs: Int,
        maxPx: Int
    ) {
        guard AVCaptureDevice.authorizationStatus(for: .video) == .authorized else {
            done(.failure(WireError(code: "not_authorized")))
            return
        }
        guard let device = AVCaptureDevice.default(for: .video) else {
            done(.failure(WireError(code: "unsupported")))
            return
        }
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("atelier-\(UUID().uuidString).mov")
        let capture = MacVideoCapture(done, url: url)
        active[ObjectIdentifier(capture)] = capture
        // Configurer et démarrer une session ne doit pas se faire sur la file principale.
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                capture.session.beginConfiguration()
                capture.session.sessionPreset = maxPx >= 1280 ? .high : .medium
                let input = try AVCaptureDeviceInput(device: device)
                if capture.session.canAddInput(input) { capture.session.addInput(input) }
                if capture.session.canAddOutput(capture.output) {
                    capture.session.addOutput(capture.output)
                }
                capture.session.commitConfiguration()
                capture.session.startRunning()
                capture.output.startRecording(to: url, recordingDelegate: capture)
                DispatchQueue.global().asyncAfter(deadline: .now() + Double(durationMs) / 1000) {
                    if capture.output.isRecording { capture.output.stopRecording() }
                }
            } catch {
                capture.finish(.failure(WireError(code: "unsupported")))
            }
        }
    }

    private func finish(_ result: Result<VideoAnswer, WireError>) {
        guard !finished else { return }
        finished = true
        if session.isRunning { session.stopRunning() }
        MacVideoCapture.active[ObjectIdentifier(self)] = nil
        let done = self.done
        self.done = nil
        DispatchQueue.main.async { done?(result) }
    }

    func fileOutput(
        _ output: AVCaptureFileOutput,
        didFinishRecordingTo outputFileURL: URL,
        from connections: [AVCaptureConnection],
        error: Error?
    ) {
        if error != nil {
            finish(.failure(WireError(code: "internal")))
            return
        }
        let size = (try? FileManager.default.attributesOfItem(atPath: outputFileURL.path)[.size]
            as? NSNumber).flatMap { $0 }?.intValue ?? 0
        guard size > 0 else {
            finish(.failure(WireError(code: "internal")))
            return
        }
        finish(
            .success(
                VideoAnswer(
                    path: outputFileURL.path,
                    bytes: size,
                    duration_ms: Int(output.recordedDuration.seconds * 1000)
                )
            )
        )
    }
}

/// A bounded audio recording. macOS has no `AVAudioSession`, so the iOS session juggling is
/// absent; `AVAudioRecorder` alone does the recording.
private final class MacAudioRecorder: NSObject {
    static let shared = MacAudioRecorder()

    private var recorder: AVAudioRecorder?
    private var url: URL?
    private var startedAt: Date?
    private var finished: AudioAnswer?

    func start(maxMs: Int) -> Result<Int, WireError> {
        if recorder != nil {
            return .failure(WireError(code: "unavailable"))
        }
        guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
            return .failure(WireError(code: "not_authorized"))
        }
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("atelier-\(UUID().uuidString).m4a")
        let settings: [String: Any] = [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: 22_050,
            AVNumberOfChannelsKey: 1,
            AVEncoderAudioQualityKey: AVAudioQuality.medium.rawValue,
        ]
        do {
            let recorder = try AVAudioRecorder(url: url, settings: settings)
            guard recorder.record(forDuration: Double(maxMs) / 1000) else {
                return .failure(WireError(code: "unsupported"))
            }
            recorder.delegate = self
            self.recorder = recorder
            self.url = url
            self.startedAt = Date()
            self.finished = nil
            return .success(maxMs)
        } catch {
            return .failure(WireError(code: "unsupported"))
        }
    }

    func stop() -> Result<AudioAnswer, WireError> {
        if let done = finished {
            finished = nil
            return .success(done)
        }
        guard let recorder = recorder, let url = url else {
            return .failure(WireError(code: "nothing_recording"))
        }
        let seconds = elapsed(recorder)
        recorder.stop()
        self.recorder = nil
        self.url = nil
        self.startedAt = nil

        let size = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)
            .flatMap { $0 }?.uint64Value ?? 0
        if size == 0 {
            return .failure(WireError(code: "internal"))
        }
        return .success(
            AudioAnswer(path: url.path, bytes: size, duration_ms: UInt64((seconds * 1000).rounded()))
        )
    }

    private func elapsed(_ recorder: AVAudioRecorder) -> TimeInterval {
        guard let startedAt = startedAt else { return recorder.currentTime }
        return Date().timeIntervalSince(startedAt)
    }

    func didFinishOnItsOwn() {
        guard let recorder = recorder, let url = url else { return }
        let seconds = elapsed(recorder)
        let size = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)
            .flatMap { $0 }?.uint64Value ?? 0
        recorder.stop()
        self.recorder = nil
        self.url = nil
        self.startedAt = nil
        if size > 0 {
            finished = AudioAnswer(
                path: url.path,
                bytes: size,
                duration_ms: UInt64((seconds * 1000).rounded())
            )
        }
    }
}

extension MacAudioRecorder: AVAudioRecorderDelegate {
    func audioRecorderDidFinishRecording(_ recorder: AVAudioRecorder, successfully flag: Bool) {
        DispatchQueue.main.async { [weak self] in
            self?.didFinishOnItsOwn()
        }
    }
}

// MARK: - Capture commands (called from dispatch.swift)

extension MacCameraMic {
    /// `cameraCapturePhoto` — Args: `{"max_px": 1280, "quality": 0.8}`.
    static func capturePhoto(_ args: [String: Any]) -> String {
        let maxPx = min(max((args["max_px"] as? Int) ?? 1280, 64), 4096)
        let quality = min(max((args["quality"] as? Double) ?? 0.8, 0.1), 1.0)
        return blockingCall(timeout: 15) { done in
            MacCameraCapture.shared.capture(maxPx: maxPx, quality: quality) { result in
                switch result {
                case .success(let photo): done(encodeJson(photo))
                case .failure(let failure): done(errorReply(code: failure.code))
                }
            }
        }
    }

    /// `cameraRecordVideo` — Args: `{"duration_ms": 5000, "max_px": 1280}`.
    static func recordVideo(_ args: [String: Any]) -> String {
        let durationMs = min(max((args["duration_ms"] as? Int) ?? 5_000, 1_000), 30_000)
        let maxPx = min(max((args["max_px"] as? Int) ?? 1280, 64), 4096)
        return blockingCall(timeout: Double(durationMs) / 1000 + 10) { done in
            MacVideoCapture.run({ result in
                switch result {
                case .success(let video): done(encodeJson(video))
                case .failure(let failure): done(errorReply(code: failure.code))
                }
            }, durationMs: durationMs, maxPx: maxPx)
        }
    }

    /// `microphoneStart` — Args: `{"max_ms": 60000}`. Renders at once; the recording stops on
    /// `microphoneStop`, or alone at the bound.
    static func microphoneStart(_ args: [String: Any]) -> String {
        let maxMs = min(max((args["max_ms"] as? Int) ?? 60_000, 500), 60_000)
        return onMainSync {
            switch MacAudioRecorder.shared.start(maxMs: maxMs) {
            case .success(let maxMs): return encodeJson(RecordingAnswer(max_ms: maxMs))
            case .failure(let failure): return errorReply(code: failure.code)
            }
        }
    }

    /// `microphoneStop` — collect the recording, or the one that stopped on its own.
    static func microphoneStop(_ args: [String: Any]) -> String {
        return onMainSync {
            switch MacAudioRecorder.shared.stop() {
            case .success(let audio): return encodeJson(audio)
            case .failure(let failure): return errorReply(code: failure.code)
            }
        }
    }
}
