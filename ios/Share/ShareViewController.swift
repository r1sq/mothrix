// SPDX-License-Identifier: MPL-2.0
//
// The share extension: "mothrix" in the share sheet of other apps. It shows
// the rooms the app last wrote into the shared folder (`share-rooms.json`;
// the extension does not sync), and sends what was shared — pictures,
// files, links, text — through Rust's short-lived client on the shared
// store (`share`), once the tapped room is confirmed (a mistap sends
// nothing).

import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// What the app writes for the extension: rooms, most recent first, and
/// its colours.
struct ShareList: Decodable {
    struct Room: Decodable, Identifiable {
        var id: String
        var name: String
        var isDm: Bool
        var account: String
    }

    var rooms: [Room]
    var background: String
    var fg: String
    var dim: String
    var accent: String
    var error: String
}

final class ShareViewController: UIViewController {
    override func viewDidLoad() {
        super.viewDidLoad()
        let model = ShareModel(context: extensionContext)
        let host = UIHostingController(rootView: ShareView(model: model))
        addChild(host)
        host.view.frame = view.bounds
        host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(host.view)
        host.didMove(toParent: self)
        model.load()
    }
}

@MainActor
final class ShareModel: ObservableObject {
    @Published var list: ShareList?
    @Published var text = ""
    @Published var files: [URL] = []
    @Published var sending: String?
    @Published var problem: String?
    private let context: NSExtensionContext?

    init(context: NSExtensionContext?) { self.context = context }

    var folder: URL {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: "group.de.tmrk.mothrix")
            ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
    }

    func load() {
        if let data = try? Data(contentsOf: folder.appendingPathComponent("share-rooms.json")) {
            let decoder = JSONDecoder()
            decoder.keyDecodingStrategy = .convertFromSnakeCase
            list = try? decoder.decode(ShareList.self, from: data)
        }
        let providers = (context?.inputItems as? [NSExtensionItem] ?? [])
            .flatMap { $0.attachments ?? [] }
        Task {
            var texts: [String] = []
            for provider in providers {
                if let file = await Self.file(from: provider) {
                    files.append(file)
                } else if let line = await Self.text(from: provider) {
                    texts.append(line)
                }
            }
            text = texts.joined(separator: "\n")
        }
    }

    func send(to room: ShareList.Room) {
        sending = room.name
        let folder = folder.path
        let text = text
        let paths = files.map(\.path)
        Task.detached {
            StoreKey.handOver()
            do {
                try share(folder: folder, room: room.id, text: text, files: paths)
                await MainActor.run {
                    self.tidy()
                    self.context?.completeRequest(returningItems: nil)
                }
            } catch {
                await MainActor.run {
                    self.sending = nil
                    self.problem = "Not sent: \(error)"
                }
            }
        }
    }

    func cancel() {
        tidy()
        context?.cancelRequest(withError: NSError(domain: "mothrix", code: 0))
    }

    /// Delete the copies made of what was shared (each in a folder of its
    /// own, see `file(from:)`): sent or given up, they are not needed.
    private func tidy() {
        for file in files {
            try? FileManager.default.removeItem(at: file.deletingLastPathComponent())
        }
    }

    /// A picture (made JPEG: no location goes out, every client shows it),
    /// a film or any other file, copied where Rust can read it.
    private static func file(from provider: NSItemProvider) async -> URL? {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let name = provider.suggestedName ?? (provider.hasItemConformingToTypeIdentifier(UTType.image.identifier) ? "picture" : "file")
        if provider.hasItemConformingToTypeIdentifier(UTType.image.identifier) {
            let file = folder.appendingPathComponent((name as NSString).deletingPathExtension + ".jpg")
            guard let data = try? await provider.loadData(for: .image),
                  let jpeg = UIImage(data: data)?.jpegData(compressionQuality: 0.85),
                  (try? jpeg.write(to: file)) != nil
            else {
                try? FileManager.default.removeItem(at: folder)
                return nil
            }
            return file
        }
        let isLink = provider.hasItemConformingToTypeIdentifier(UTType.url.identifier)
            && !provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier)
        let isText = provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier)
        guard !isLink, !isText,
              provider.hasItemConformingToTypeIdentifier(UTType.item.identifier)
        else {
            try? FileManager.default.removeItem(at: folder)
            return nil
        }
        let copied: URL? = await withCheckedContinuation { done in
            _ = provider.loadFileRepresentation(for: .item) { url, _, _ in
                guard let url else { return done.resume(returning: nil) }
                let copy = folder.appendingPathComponent(url.lastPathComponent)
                done.resume(returning: (try? FileManager.default.copyItem(at: url, to: copy)) != nil ? copy : nil)
            }
        }
        // Nothing came of it: its folder is not left behind.
        if copied == nil { try? FileManager.default.removeItem(at: folder) }
        return copied
    }

    /// A link or plain text, for the message.
    private static func text(from provider: NSItemProvider) async -> String? {
        if provider.canLoadObject(ofClass: URL.self),
           let url = try? await provider.loadObject(ofClass: URL.self)
        {
            return url.absoluteString
        }
        if provider.canLoadObject(ofClass: String.self),
           let text = try? await provider.loadObject(ofClass: String.self)
        {
            return text
        }
        return nil
    }
}

extension NSItemProvider {
    func loadData(for type: UTType) async throws -> Data {
        try await withCheckedThrowingContinuation { done in
            _ = loadDataRepresentation(for: type) { data, error in
                if let data { done.resume(returning: data) } else {
                    done.resume(throwing: error ?? NSError(domain: "mothrix", code: 1))
                }
            }
        }
    }

    func loadObject<T: _ObjectiveCBridgeable>(ofClass: T.Type) async throws -> T
    where T._ObjectiveCType: NSItemProviderReading {
        try await withCheckedThrowingContinuation { done in
            _ = loadObject(ofClass: T.self) { value, error in
                if let value { done.resume(returning: value) } else {
                    done.resume(throwing: error ?? NSError(domain: "mothrix", code: 1))
                }
            }
        }
    }
}

struct ShareView: View {
    @ObservedObject var model: ShareModel
    @State private var filter = ""
    /// The room tapped: asked about before anything is sent.
    @State private var confirming: ShareList.Room?

    private func color(_ hex: String?, _ fallback: Color) -> Color {
        guard let hex, let value = UInt32(hex.dropFirst(), radix: 16) else { return fallback }
        return Color(red: Double((value >> 16) & 0xff) / 255,
                     green: Double((value >> 8) & 0xff) / 255,
                     blue: Double(value & 0xff) / 255)
    }

    var body: some View {
        let bg = color(model.list?.background, .black)
        let fg = color(model.list?.fg, .white)
        let dim = color(model.list?.dim, .gray)
        let accent = color(model.list?.accent, .orange)
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("send to …").bold().foregroundStyle(accent)
                Spacer()
                Button("[cancel]") { model.cancel() }.foregroundStyle(accent)
            }
            if !model.files.isEmpty {
                Text(model.files.map(\.lastPathComponent).joined(separator: ", "))
                    .lineLimit(2).foregroundStyle(dim)
            }
            TextField("", text: $model.text, prompt: Text("message").foregroundColor(dim), axis: .vertical)
                .lineLimit(1...5)
            Rectangle().fill(dim.opacity(0.4)).frame(height: 1)
            TextField("", text: $filter, prompt: Text("find a room").foregroundColor(dim))
            if let sending = model.sending {
                HStack { ProgressView(); Text("sending to \(sending) …") }.foregroundStyle(dim)
            }
            if let problem = model.problem {
                Text(problem).foregroundStyle(color(model.list?.error, .red))
            }
            if let list = model.list {
                let several = Set(list.rooms.map(\.account)).count > 1
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 12) {
                        ForEach(rooms(list)) { room in
                            Button {
                                confirming = room
                            } label: {
                                HStack(spacing: 0) {
                                    Text(room.isDm ? "@ " : "# ").foregroundStyle(dim)
                                    Text(room.name).lineLimit(1).foregroundStyle(fg)
                                    Spacer()
                                    if several { Text(room.account).foregroundStyle(dim) }
                                }
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .disabled(model.sending != nil)
                        }
                    }
                }
            } else {
                Text("Open mothrix once, then its rooms show here.").foregroundStyle(dim)
                Spacer()
            }
        }
        .confirmationDialog(
            confirmTitle,
            isPresented: Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } }),
            titleVisibility: .visible,
            presenting: confirming
        ) { room in
            Button("send") { model.send(to: room) }
            Button("cancel", role: .cancel) {}
        } message: { _ in
            Text(what)
        }
        .font(.system(.body, design: .monospaced))
        .padding(16)
        .background(bg)
        .foregroundStyle(fg)
        .preferredColorScheme(.dark)
    }

    /// "Send to # lunch?", with the account when there are several.
    private var confirmTitle: String {
        guard let room = confirming else { return "" }
        let mark = room.isDm ? "@" : "#"
        let several = Set(model.list?.rooms.map(\.account) ?? []).count > 1
        let account = several ? " (" + room.account + ")" : ""
        return "Send to " + mark + " " + room.name + account + "?"
    }

    /// What would go: the files by name, and the start of the message.
    private var what: String {
        var parts = model.files.map(\.lastPathComponent)
        let text = model.text.trimmingCharacters(in: .whitespacesAndNewlines)
        if !text.isEmpty { parts.append(text.count > 80 ? String(text.prefix(80)) + "…" : text) }
        return parts.joined(separator: "\n")
    }

    private func rooms(_ list: ShareList) -> [ShareList.Room] {
        let query = filter.lowercased()
        return query.isEmpty ? list.rooms : list.rooms.filter { $0.name.lowercased().contains(query) }
    }
}
