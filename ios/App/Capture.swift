// SPDX-License-Identifier: MPL-2.0
//
// Taking a photo with the camera, and recording a voice message.

import AVFoundation
import SwiftUI
import UIKit

/// The system camera; hands over the photo taken, or nothing.
struct CameraPicker: UIViewControllerRepresentable {
    let taken: (UIImage?) -> Void

    static var available: Bool { UIImagePickerController.isSourceTypeAvailable(.camera) }

    func makeUIViewController(context: Context) -> UIImagePickerController {
        let picker = UIImagePickerController()
        picker.sourceType = .camera
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ picker: UIImagePickerController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(taken: taken) }

    final class Coordinator: NSObject, UIImagePickerControllerDelegate, UINavigationControllerDelegate {
        let taken: (UIImage?) -> Void
        init(taken: @escaping (UIImage?) -> Void) { self.taken = taken }

        func imagePickerController(
            _ picker: UIImagePickerController,
            didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]
        ) {
            taken(info[.originalImage] as? UIImage)
        }

        func imagePickerControllerDidCancel(_ picker: UIImagePickerController) { taken(nil) }
    }
}

/// Records a voice message as AAC (`.m4a`), and how loud it was over time
/// for the waveform other clients draw.
@MainActor
final class VoiceRecorder: ObservableObject {
    @Published private(set) var recording = false
    @Published private(set) var seconds: TimeInterval = 0
    /// Loudness, 0 to 1, ten times a second.
    @Published private(set) var levels: [Float] = []
    @Published var problem: String?

    private var recorder: AVAudioRecorder?
    private var timer: Timer?

    func start() {
        Task {
            guard await AVAudioApplication.requestRecordPermission() else {
                problem = "mothrix may not use the microphone (Settings → mothrix)."
                return
            }
            do {
                let session = AVAudioSession.sharedInstance()
                try session.setCategory(.playAndRecord, mode: .default, options: [.defaultToSpeaker])
                try session.setActive(true)
                // Its own folder, so that the name other clients show can
                // be plain.
                let folder = FileManager.default.temporaryDirectory
                    .appendingPathComponent(UUID().uuidString)
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                let file = folder.appendingPathComponent("Voice message.m4a")
                let recorder = try AVAudioRecorder(url: file, settings: [
                    AVFormatIDKey: kAudioFormatMPEG4AAC,
                    AVSampleRateKey: 44_100,
                    AVNumberOfChannelsKey: 1,
                    AVEncoderAudioQualityKey: AVAudioQuality.high.rawValue,
                ])
                recorder.isMeteringEnabled = true
                guard recorder.record() else {
                    problem = "Recording did not start."
                    return
                }
                self.recorder = recorder
                levels = []
                seconds = 0
                recording = true
                timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
                    Task { @MainActor in self?.sample() }
                }
            } catch {
                problem = "Recording did not start: \(error.localizedDescription)"
            }
        }
    }

    private func sample() {
        guard let recorder, recorder.isRecording else { return }
        recorder.updateMeters()
        // -50 dB and quieter is silence; 0 dB is as loud as it gets.
        let power = recorder.averagePower(forChannel: 0)
        levels.append(max(0, min(1, (power + 50) / 50)))
        seconds = recorder.currentTime
    }

    /// Stop; the file, its length in milliseconds and its waveform (about a
    /// hundred values), unless it was too short to mean anything.
    func finish() -> (url: URL, durationMs: UInt64, waveform: [Float])? {
        guard let recorder else { return nil }
        let length = recorder.currentTime
        stop()
        guard length >= 0.5 else {
            try? FileManager.default.removeItem(at: recorder.url)
            return nil
        }
        return (recorder.url, UInt64(length * 1000), Self.shrink(levels, to: 100))
    }

    func cancel() {
        guard let recorder else { return }
        stop()
        try? FileManager.default.removeItem(at: recorder.url)
    }

    private func stop() {
        timer?.invalidate()
        timer = nil
        recorder?.stop()
        recorder = nil
        recording = false
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    /// The loudest of each stretch, so that `count` values cover it all.
    static func shrink(_ levels: [Float], to count: Int) -> [Float] {
        guard levels.count > count else { return levels }
        let size = Double(levels.count) / Double(count)
        return (0..<count).map { index in
            let from = Int(Double(index) * size)
            let to = max(from + 1, Int(Double(index + 1) * size))
            return levels[from..<min(to, levels.count)].max() ?? 0
        }
    }
}

/// In place of the message box while recording: the time, the last
/// moments' loudness, and cancel or send.
struct VoiceBar: View {
    @EnvironmentObject var store: Store
    @ObservedObject var recorder: VoiceRecorder
    let send: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Text("●").foregroundStyle(store.theme.errorColor)
            Text(String(format: "%d:%02d", Int(recorder.seconds) / 60, Int(recorder.seconds) % 60))
                .monospacedDigit()
            Text(bars).foregroundStyle(store.theme.accentColor).lineLimit(1)
            Spacer()
            WordButton("[cancel]") { recorder.cancel() }
            WordButton("[send]", action: send)
        }
        .font(.mono())
    }

    /// The last levels as block characters, newest at the right.
    private var bars: String {
        let blocks = Array("▁▂▃▄▅▆▇█")
        return String(recorder.levels.suffix(16).map { level in
            blocks[min(blocks.count - 1, Int(level * Float(blocks.count)))]
        })
    }
}
