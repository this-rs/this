// One audio recording, bounded in duration (AVAudioRecorder).
//
// The file, not the bytes: a minute of speech is a few hundred kilobytes, and the bridge keeps
// facts, not payloads (`families/camera_mic.rs`, `Audio`). The recording is written to the app's
// temporary directory and its path is handed back, so whoever needs the content reads it
// through the document path like any other file.
//
// NO TAURI IMPORT: type-checked with the iOS SDK alongside `CameraCapture.swift`.

import AVFoundation
import Foundation

/// One recording, as the Rust side reads it (`families/camera_mic.rs`, `Audio`).
struct RecordedAudio: Encodable {
  /// The wire tag: the Rust side reads `{"kind":"audio",…}`.
  let kind = "audio"
  let path: String
  let bytes: UInt64
  let duration_ms: UInt64
}

final class AudioRecorder: NSObject {
  static let shared = AudioRecorder()

  private var recorder: AVAudioRecorder?
  private var url: URL?
  /// The start, kept because `currentTime` reads zero once the recorder has stopped — and the
  /// bound stops it on its own, which is exactly the case a caller cannot see.
  private var startedAt: Date?
  /// The result of the recording that just ended, waiting to be collected by `stop`.
  ///
  /// `record(forDuration:)` stops by itself at the bound: without this, a caller that waits
  /// (an agent recording a one-minute note, then asking for the file) would find nothing.
  private var finished: RecordedAudio?

  /// Start recording. Answers the bound the file will respect, or a refusal.
  func start(maxMs: Int) -> Result<Int, CaptureFailure> {
    if recorder != nil {
      // One recording at a time: two would write two files and neither would be the answer.
      return .failure(.code("unavailable"))
    }
    guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
      return .failure(.code("not_authorized"))
    }
    let session = AVAudioSession.sharedInstance()
    do {
      try session.setCategory(.playAndRecord, mode: .default, options: [.defaultToSpeaker])
      try session.setActive(true)
    } catch {
      return .failure(.code("unsupported"))
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
        return .failure(.code("unsupported"))
      }
      recorder.delegate = self
      self.recorder = recorder
      self.url = url
      self.startedAt = Date()
      self.finished = nil
      return .success(maxMs)
    } catch {
      return .failure(.code("unsupported"))
    }
  }

  /// Stop (or collect) the recording. Answers the file, with its real size and duration.
  func stop() -> Result<RecordedAudio, CaptureFailure> {
    if let done = finished {
      finished = nil
      return .success(done)
    }
    guard let recorder = recorder, let url = url else {
      return .failure(.code("nothing_recording"))
    }
    let seconds = elapsed(recorder)
    recorder.stop()
    self.recorder = nil
    self.url = nil
    self.startedAt = nil
    try? AVAudioSession.sharedInstance().setActive(false, options: [.notifyOthersOnDeactivation])

    let size = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)
      .flatMap { $0 }?.uint64Value ?? 0
    if size == 0 {
      return .failure(.code("internal"))
    }
    let audio = RecordedAudio(
      path: url.path,
      bytes: size,
      duration_ms: UInt64((seconds * 1000).rounded())
    )
    return .success(audio)
  }

  /// How long the recording ran. Read from the clock, because the recorder zeroes its own
  /// counter when it stops — which is what happens at the bound.
  private func elapsed(_ recorder: AVAudioRecorder) -> TimeInterval {
    guard let startedAt = startedAt else { return recorder.currentTime }
    return Date().timeIntervalSince(startedAt)
  }

  /// Called when the bound was reached on its own: keep the result for the next `stop`.
  func didFinishOnItsOwn() {
    guard let recorder = recorder, let url = url else { return }
    let seconds = elapsed(recorder)
    let size = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)
      .flatMap { $0 }?.uint64Value ?? 0
    recorder.stop()
    self.recorder = nil
    self.url = nil
    self.startedAt = nil
    try? AVAudioSession.sharedInstance().setActive(false, options: [.notifyOthersOnDeactivation])
    if size > 0 {
      finished = RecordedAudio(
        path: url.path,
        bytes: size,
        duration_ms: UInt64((seconds * 1000).rounded())
      )
    }
  }
}

extension AudioRecorder: AVAudioRecorderDelegate {
  func audioRecorderDidFinishRecording(_ recorder: AVAudioRecorder, successfully flag: Bool) {
    // The delegate runs on the thread that started the recording; the state it touches is only
    // read from the selectors, which run on the main thread.
    DispatchQueue.main.async { [weak self] in
      self?.didFinishOnItsOwn()
    }
  }
}
