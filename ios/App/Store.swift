// SPDX-License-Identifier: MPL-2.0
//
// The app's side of mothrix-ffi: starts the accounts in Rust, keeps what
// the events say for SwiftUI, and sends what the user does as actions.
// All decisions are Rust's (`view.rs`); this only stores and asks.

import Foundation
import SwiftUI
import UserNotifications
import UIKit

/// The folder the app and its notification extension share.
let appGroup = "group.de.tmrk.mothrix"

/// Where the homeserver sends pushes for this app (Sygnal), and which of
/// Sygnal's apps: builds from Xcode push through Apple's sandbox.
let pushGateway = "https://push.tmrk.de/_matrix/push/v1/notify"
#if DEBUG
let pushAppId = "de.tmrk.mothrix.dev"
#else
let pushAppId = "de.tmrk.mothrix"
#endif

func sharedFolder() -> URL {
    FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup)
        ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
}

/// Hands Rust's events to the store. Decoding (JSON, pictures) happens
/// here, on Rust's thread, in order; only the result goes to the main
/// thread, which a picture's megabyte of base64 would otherwise hold up.
final class EventSink: Listener, @unchecked Sendable {
    weak var store: Store?

    func onEvent(json: String) {
        let decoded = Result { try JSONDecoder.mothrix.decode(MothrixEvent.self, from: Data(json.utf8)) }
        var picture: (String, UIImage)?
        if case .success(.picture(let eventId, let url)) = decoded,
           let image = Store.image(dataURL: url)
        {
            picture = (eventId, image.preparingForDisplay() ?? image)
        }
        DispatchQueue.main.async { [weak self] in
            guard let store = self?.store else { return }
            switch decoded {
            case .success(let event):
                if let (eventId, image) = picture {
                    store.pictures[eventId] = image
                } else {
                    store.handle(event)
                }
            case .failure(let error):
                store.couldNotRead(error)
            }
        }
    }
}

@MainActor
final class Store: ObservableObject {
    @Published var accounts: [Account] = []
    @Published var rooms: [RoomEntry] = []
    @Published var lines: [Line] = []
    @Published var openRoom: String?
    @Published var thread: String?
    @Published var items: [Message] = []
    @Published var pictures: [String: UIImage] = [:]
    @Published var typing: [String] = []
    @Published var theme: Palette = .fallback
    @Published var themes: [String] = []
    @Published var prefs: Prefs?
    @Published var sections: [String] = []
    @Published var accountFilter: Int?
    @Published var unreadOnly = false
    @Published var searchQuery = ""
    @Published var searchHits: [SearchHit] = []
    @Published var roomDetails: RoomDetails?
    @Published var verify: VerifyState?
    @Published var recoveryKey: String?
    @Published var emojiGroups: [EmojiGroup] = []
    /// An attachment downloaded to be shown (QuickLook), and the one being
    /// fetched.
    /// A message to scroll to and mark for a moment (a reply's original, a
    /// search hit).
    @Published var revealed: String?
    @Published var mediaFile: URL?
    @Published var fetchingMedia: String?
    /// What is behind links: title and summary, by address.
    @Published var previews: [String: (title: String, description: String)] = [:]
    private var askedPreviews = Set<String>()
    /// Unsent text per room (and thread), kept over restarts.
    private var drafts: [String: String] =
        (UserDefaults(suiteName: appGroup)?.dictionary(forKey: "drafts") as? [String: String]) ?? [:]
    /// Who is in the open room, for @-mentions.
    @Published var members: [Member] = []
    @Published var signIn = SignInState()
    @Published var favourites: [String] = ["👍", "❤️", "😂"]
    @Published var notice: String?
    @Published var started = false
    /// The push token, once iOS gave it (base64).
    private var pushToken: String?
    private var mothrix: Mothrix?
    private let sink = EventSink()

    func start() {
        guard mothrix == nil else { return }
        sink.store = self
        do {
            mothrix = try Mothrix(folder: sharedFolder().path, listener: sink)
        } catch {
            notice = "mothrix could not start: \(error)"
        }
    }

    func couldNotRead(_ error: Error) {
        print("mothrix: an event could not be read: \(error)")
        #if DEBUG
        notice = "event not understood: \(error)"
        #endif
    }

    func handle(_ event: MothrixEvent) {
        switch event {
        case .snapshot(let snapshot):
            accounts = snapshot.accounts
            rooms = snapshot.rooms
            lines = snapshot.lines
            openRoom = snapshot.openRoom
            thread = snapshot.thread
            items = snapshot.items
            theme = snapshot.settings.theme
            themes = snapshot.settings.themes
            prefs = snapshot.settings.prefs
            sections = snapshot.settings.sections
            accountFilter = snapshot.settings.accountFilter
            unreadOnly = snapshot.settings.unreadOnly
            signIn = snapshot.signIn
            favourites = snapshot.favourites
            emojiGroups = snapshot.emoji
            started = true
            registerPush()
        case .accounts(let list):
            let added = list.count > accounts.count
            accounts = list
            if added { registerPush() }
        case .rooms(let list):
            rooms = list
            notificationsFollowRooms()
        case .sidebar(let list): lines = list
        case .opened(let room, let thread):
            openRoom = room
            if let room { Self.clearNotifications(of: [room]) }
            self.thread = thread
        case .timeline(let room, let thread, let list):
            // Rust only sends the open room or thread, and may do so
            // before the `opened` event that says which it is.
            openRoom = room
            self.thread = thread
            items = list
        case .picture: break  // Decoded by the sink, off the main thread.
        case .typing(let room, let names):
            if room == openRoom { typing = names }
        case .notice(let text): notice = text
        case .settings(let settings):
            theme = settings.theme
            themes = settings.themes
            prefs = settings.prefs
            sections = settings.sections
            accountFilter = settings.accountFilter
            unreadOnly = settings.unreadOnly
        case .searchResults(let query, let hits):
            searchQuery = query
            searchHits = hits
        case .roomDetails(let details): roomDetails = details
        case .verify(let state): verify = state
        case .reveal(let eventId): revealed = eventId
        case .recoveryKey(let key): recoveryKey = key
        case .members(let room, let list): if room == openRoom { members = list }
        case .media(let eventId, let path):
            if fetchingMedia == eventId {
                fetchingMedia = nil
                mediaFile = URL(fileURLWithPath: path)
            }
        case .linkPreview(let url, let title, let description):
            previews[url] = (title, description)
        case .signIn(let state): signIn = state
        case .favourites(let emoji): favourites = emoji
        case .notify: break  // In the foreground the room list shows it.
        case .other: break
        }
    }

    /// A `data:image/…;base64,…` address as a picture.
    nonisolated static func image(dataURL: String) -> UIImage? {
        guard let comma = dataURL.firstIndex(of: ",") else { return nil }
        guard let data = Data(base64Encoded: String(dataURL[dataURL.index(after: comma)...]))
        else { return nil }
        return UIImage(data: data)
    }

    // What the user does.

    func act(_ type: String, _ fields: [String: Any] = [:]) {
        var action = fields
        action["type"] = type
        guard let data = try? JSONSerialization.data(withJSONObject: action),
              let json = String(data: data, encoding: .utf8)
        else { return }
        do {
            try mothrix?.act(json: json)
        } catch {
            notice = "\(error)"
        }
    }

    func open(_ room: String) { act("open", ["room": room]) }
    func send(_ text: String, mentions: [Member] = []) {
        act("send", ["text": text, "mentions": Self.mentionList(mentions)])
    }
    func reply(to eventId: String, _ text: String, mentions: [Member] = []) {
        act("reply", ["event_id": eventId, "text": text, "mentions": Self.mentionList(mentions)])
    }

    /// The people completed with @; Rust keeps those whose name is still
    /// in the text.
    private static func mentionList(_ mentions: [Member]) -> [[String: String]] {
        mentions.map { ["user_id": $0.userId, "name": $0.name] }
    }

    func createPoll(question: String, answers: [String], multiple: Bool, hidden: Bool) {
        act("create_poll", ["question": question, "answers": answers, "multiple": multiple, "hidden": hidden])
    }

    /// An emoji was put into a message: it counts among the favourites.
    func usedEmoji(_ emoji: String) { act("used_emoji", ["emoji": emoji]) }
    func react(_ eventId: String, _ key: String) {
        act("react", ["event_id": eventId, "key": key])
    }
    func vote(_ eventId: String, _ answer: String) {
        act("vote", ["event_id": eventId, "answer": answer])
    }
    func loadOlder() { act("load_older") }
    func fold(_ key: String) { act("fold", ["key": key]) }
    func typing(_ active: Bool) { act("typing", ["active": active]) }
    /// Fetch an attachment, then show it (`.media` comes back).
    func openMedia(_ eventId: String) {
        fetchingMedia = eventId
        act("open_media", ["event_id": eventId])
    }

    /// Ask once what is behind a link (only if link previews are on).
    func previewLink(_ url: String) {
        guard prefs?.linkPreviews == true, askedPreviews.insert(url).inserted else { return }
        act("preview_link", ["url": url])
    }

    /// Scroll to a message of the open room; Rust pages back to it first if
    /// it is not loaded (`.reveal` comes then).
    func reveal(_ eventId: String) {
        guard !eventId.isEmpty else { return }
        if items.contains(where: { $0.eventId == eventId }) {
            revealed = eventId
        } else {
            act("reveal", ["event_id": eventId])
        }
    }

    func markRead(_ room: String) { act("mark_read", ["room": room]) }

    func togglePin(_ eventId: String) { act("toggle_pin", ["event_id": eventId]) }
    func endPoll(_ eventId: String) { act("end_poll", ["event_id": eventId]) }
    func forward(_ eventId: String, to room: String) {
        act("forward", ["event_id": eventId, "room": room])
    }

    /// The draft of a room or thread, and keeping one.
    func draft(_ key: String) -> String { drafts[key] ?? "" }
    func keepDraft(_ key: String, _ text: String) {
        if text.isEmpty { drafts.removeValue(forKey: key) } else { drafts[key] = text }
        UserDefaults(suiteName: appGroup)?.set(drafts, forKey: "drafts")
    }

    // The room list.
    func setFavourite(_ room: String, _ on: Bool) { act("favourite", ["room": room, "value": on]) }
    func setLowPriority(_ room: String, _ on: Bool) { act("low_priority", ["room": room, "value": on]) }
    func setNotify(_ room: String, _ mode: String) { act("notify", ["room": room, "mode": mode]) }
    func move(_ room: String, toSection section: String?) {
        act("move_to_section", ["room": room, "section": section ?? NSNull()])
    }
    func leave(_ room: String) { act("leave", ["room": room]) }
    func showAccount(_ account: Int?) { act("show_account", ["account": account ?? NSNull()]) }
    func setUnreadOnly(_ on: Bool) { act("unread_only", ["value": on]) }
    func startChat(account: Int, userId: String) {
        act("start_dm", ["account": account, "user_id": userId])
    }
    func createRoom(account: Int, name: String) { act("create_room", ["account": account, "name": name]) }
    func joinRoom(account: Int, address: String) { act("join", ["account": account, "target": address]) }

    func signIn(server: String, user: String, password: String) {
        act("sign_in", ["server": server, "user": user, "password": password, "name": ""])
    }
    /// A recorded voice message (a file of the app's), into the open room.
    func sendVoice(_ file: URL, durationMs: UInt64, waveform: [Float]) {
        act("send_voice", ["path": file.path, "duration_ms": durationMs, "waveform": waveform])
    }

    func sendPicture(_ data: Data, name: String, caption: String = "") {
        sendFile(data, name: name, caption: caption)
    }

    func edit(_ eventId: String, _ text: String, mentions: [Member] = []) {
        act("edit", ["event_id": eventId, "text": text, "mentions": Self.mentionList(mentions)])
    }
    func delete(_ eventId: String) { act("delete", ["event_id": eventId]) }

    func search(_ text: String, everywhere: Bool) {
        act("search", ["text": text, "everywhere": everywhere])
    }
    func showHit(_ hit: SearchHit) {
        act("show_hit", ["room": hit.roomId, "event_id": hit.eventId])
    }

    func roomInfo() {
        roomDetails = nil  // Not the last room's while this one's comes.
        if let room = openRoom { act("room_details", ["room": room]) }
    }

    /// Changed preferences, sent back whole in Rust's spelling.
    func setPrefs(_ changed: Prefs) {
        prefs = changed
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        guard let data = try? encoder.encode(changed),
              let object = try? JSONSerialization.jsonObject(with: data)
        else { return }
        act("prefs", ["prefs": object])
    }

    /// Any file, under its own name.
    func sendFile(_ data: Data, name: String, caption: String = "") {
        let caption = caption.trimmingCharacters(in: .whitespacesAndNewlines)
        act("upload", ["name": name, "data": data.base64EncodedString(),
                       "caption": caption.isEmpty ? NSNull() : caption])
    }

    func focus(_ focused: Bool) { mothrix?.focus(focused: focused) }
    func openFromNotification(_ room: String) { mothrix?.openFromNotification(room: room) }

    /// iOS gave the app its push token: every account's homeserver gets
    /// it (again: tokens can change, and new accounts need it too). Sygnal
    /// wants it as base64 (it makes hex of it for Apple itself).
    func setPushToken(_ token: Data) {
        pushToken = token.base64EncodedString()
        registerPush()
    }

    private func registerPush() {
        guard let token = pushToken else { return }
        let lang = Locale.current.language.languageCode?.identifier ?? "en"
        for index in accounts.indices {
            act("register_push", [
                "account": index, "token": token, "app_id": pushAppId,
                "gateway": pushGateway, "device": UIDevice.current.name, "lang": lang,
            ])
        }
    }

    // For the views.

    /// Which mothrix this is: the version (with the commit for a build
    /// that is not a release) and the app's build number.
    var versionText: String {
        "\(mothrixVersion()), build \(Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") ?? "")"
    }

    func room(_ key: String?) -> RoomEntry? { rooms.first { $0.id == key } }

    /// The logs of the app and its extensions in one file to send: the last
    /// part of each (they only grow), newest at the end.
    func logFile() -> URL? {
        let folder = sharedFolder()
        var all = "mothrix \(versionText), iOS \(UIDevice.current.systemVersion)\n"
        for part in ["app", "nse", "action", "share"] {
            guard let data = try? Data(contentsOf: folder.appendingPathComponent("mothrix-\(part).log"))
            else { continue }
            let tail = data.suffix(400_000)
            all += "\n===== mothrix-\(part).log =====\n" + String(decoding: tail, as: UTF8.self)
        }
        let file = FileManager.default.temporaryDirectory.appendingPathComponent("mothrix-log.txt")
        return (try? all.write(to: file, atomically: true, encoding: .utf8)) != nil ? file : nil
    }

    private var badge = -1

    /// The app icon's number: rooms with something new (as Synapse counts
    /// for pushes), muted ones left out. Notifications of rooms that have
    /// nothing new any more (read here, or on another device) go.
    private func notificationsFollowRooms() {
        let new = rooms.filter { ($0.unread > 0 || $0.mentions > 0) && $0.notify != "Mute" }
        if new.count != badge {
            badge = new.count
            UNUserNotificationCenter.current().setBadgeCount(badge)
        }
        let read = Set(rooms.filter { $0.unread == 0 && $0.mentions == 0 }.map(\.id))
        Self.clearNotifications(of: read)
        writeShareList()
    }

    private var shareList = Data()

    /// The rooms (and colours) for the share extension, which does not sync
    /// itself: `share-rooms.json` in the shared folder, when it changed.
    private func writeShareList() {
        let list: [String: Any] = [
            "rooms": rooms.filter { !$0.isInvite }.map { room in
                [
                    "id": room.id, "name": room.name, "is_dm": room.isDm,
                    "account": room.account < accounts.count ? accounts[room.account].label : "",
                ] as [String: Any]
            },
            "background": theme.background, "fg": theme.fg, "dim": theme.dim,
            "accent": theme.accent, "error": theme.error,
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: list, options: [.sortedKeys]),
              data != shareList
        else { return }
        shareList = data
        try? data.write(to: sharedFolder().appendingPathComponent("share-rooms.json"), options: .atomic)
    }

    /// Take the notifications of these rooms off the lock screen and out of
    /// the notification centre (the extension puts the room's key in each
    /// as its thread).
    static func clearNotifications(of rooms: Set<String>) {
        guard !rooms.isEmpty else { return }
        let center = UNUserNotificationCenter.current()
        center.getDeliveredNotifications { delivered in
            let gone = delivered
                .filter { rooms.contains($0.request.content.threadIdentifier) }
                .map(\.request.identifier)
            if !gone.isEmpty { center.removeDeliveredNotifications(withIdentifiers: gone) }
        }
    }
}
