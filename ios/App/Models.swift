// SPDX-License-Identifier: MPL-2.0
//
// What Rust sends, as Swift values. The JSON comes from mothrix-ffi's
// `view.rs` (`Event`, `Snapshot` and what they hold); keys are snake_case
// there and camelCase here (`JSONDecoder.mothrix`). Fields the app does not
// use yet are simply not declared: decoding skips them. No dictionaries
// keyed by ids: the key strategy would rename their keys.

import Foundation

extension JSONDecoder {
    static var mothrix: JSONDecoder {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }
}

/// One event from Rust, by its `"type"`.
enum MothrixEvent: Decodable {
    case snapshot(Snapshot)
    case accounts([Account])
    case rooms([RoomEntry])
    case sidebar([Line])
    case opened(room: String?, thread: String?)
    /// `dropped`: how many of its messages could not be read.
    case timeline(room: String, thread: String?, items: [Message], dropped: Int)
    case picture(eventId: String, url: String)
    case typing(room: String, names: [String])
    case notice(String)
    case settings(Settings)
    case signIn(SignInState)
    case favourites([String])
    case notify(room: String, title: String)
    case searchResults(query: String, hits: [SearchHit])
    case roomDetails(RoomDetails)
    case verify(VerifyState?)
    case reveal(eventId: String)
    case recoveryKey(String)
    case members(room: String, members: [Member])
    case media(eventId: String, path: String)
    case linkPreview(url: String, title: String, description: String)
    case problems([Problem])
    /// Something the app does not show yet.
    case other(String)

    private enum Keys: String, CodingKey {
        case type, snapshot, accounts, rooms, lines, room, thread, items, eventId, url, names,
             text, emoji, title, query, hits, details, state, key, members, path, description,
             problems
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        let type = try c.decode(String.self, forKey: .type)
        switch type {
        case "snapshot": self = .snapshot(try c.decode(Snapshot.self, forKey: .snapshot))
        case "accounts": self = .accounts(try c.decode([Account].self, forKey: .accounts))
        case "rooms": self = .rooms(try c.decode([RoomEntry].self, forKey: .rooms))
        case "sidebar": self = .sidebar(try c.decode([Line].self, forKey: .lines))
        case "opened":
            self = .opened(
                room: try c.decodeIfPresent(String.self, forKey: .room),
                thread: try c.decodeIfPresent(String.self, forKey: .thread))
        case "timeline":
            let items = try c.decode(Lossy<Message>.self, forKey: .items)
            self = .timeline(
                room: try c.decode(String.self, forKey: .room),
                thread: try c.decodeIfPresent(String.self, forKey: .thread),
                items: items.kept, dropped: items.dropped)
        case "picture":
            self = .picture(
                eventId: try c.decode(String.self, forKey: .eventId),
                url: try c.decode(String.self, forKey: .url))
        case "typing":
            self = .typing(
                room: try c.decode(String.self, forKey: .room),
                names: try c.decode([String].self, forKey: .names))
        case "notice": self = .notice(try c.decode(String.self, forKey: .text))
        case "settings": self = .settings(try Settings(from: decoder))
        case "sign_in": self = .signIn(try SignInState(from: decoder))
        case "favourites": self = .favourites(try c.decode([String].self, forKey: .emoji))
        case "notify":
            self = .notify(
                room: try c.decode(String.self, forKey: .room),
                title: try c.decode(String.self, forKey: .title))
        case "search_results":
            self = .searchResults(
                query: try c.decode(String.self, forKey: .query),
                hits: try c.decode([SearchHit].self, forKey: .hits))
        case "room_details": self = .roomDetails(try c.decode(RoomDetails.self, forKey: .details))
        case "verify": self = .verify(try c.decodeIfPresent(VerifyState.self, forKey: .state))
        case "reveal": self = .reveal(eventId: try c.decode(String.self, forKey: .eventId))
        case "recovery_key": self = .recoveryKey(try c.decode(String.self, forKey: .key))
        case "media":
            self = .media(
                eventId: try c.decode(String.self, forKey: .eventId),
                path: try c.decode(String.self, forKey: .path))
        case "link_preview":
            self = .linkPreview(
                url: try c.decode(String.self, forKey: .url),
                title: try c.decode(String.self, forKey: .title),
                description: try c.decode(String.self, forKey: .description))
        case "members":
            self = .members(
                room: try c.decode(String.self, forKey: .room),
                members: try c.decode([Member].self, forKey: .members))
        case "problems": self = .problems(try c.decode([Problem].self, forKey: .problems))
        default: self = .other(type)
        }
    }
}

/// A list read item by item: one that cannot be read (a number too large
/// for this app, a kind it does not know) is left out and counted, and
/// the others stay. A conversation is not lost to one message.
struct Lossy<Element: Decodable>: Decodable {
    var kept: [Element] = []
    var dropped = 0

    /// Reads any value and keeps nothing: to step over an item.
    private struct Skipped: Decodable {}

    init(from decoder: Decoder) throws {
        var list = try decoder.unkeyedContainer()
        while !list.isAtEnd {
            if let item = try? list.decode(Element.self) {
                kept.append(item)
            } else {
                // A failed read does not move on by itself.
                _ = try list.decode(Skipped.self)
                dropped += 1
            }
        }
    }
}

struct Snapshot: Decodable {
    var accounts: [Account]
    var rooms: [RoomEntry]
    var lines: [Line]
    var openRoom: String?
    var thread: String?
    /// The conversation's messages (`items` in the JSON), read one by one.
    private var messages: Lossy<Message>
    var items: [Message] { messages.kept }
    /// How many of them could not be read.
    var dropped: Int { messages.dropped }
    // Not `pictures`: the key strategy would rename its keys (event ids
    // with "_"). Pictures come again as `picture` events.
    var settings: Settings
    var signIn: SignInState
    var problems: [Problem]
    var favourites: [String]
    var emoji: [EmojiGroup]

    private enum CodingKeys: String, CodingKey {
        case accounts, rooms, lines, openRoom, thread
        case messages = "items"
        case settings, signIn, problems, favourites, emoji
    }
}

/// An account that is on the phone but did not start: its profile, and
/// the line that says why.
struct Problem: Decodable, Hashable {
    var profile: String
    var text: String
}

/// Every emoji of one group: `[emoji, the words it is found by]`.
struct EmojiGroup: Decodable {
    var title: String
    var emoji: [[String]]
}

/// Someone who can be @-mentioned.
struct Member: Codable, Hashable {
    var userId: String
    var name: String
}

struct Account: Decodable, Hashable {
    var profile: String
    var label: String
    var userId: String
    var ownName: String?
    var sync: SyncStatus
    var verified: Bool?
}

/// `"Online"`, or `{"Error": "…"}`.
enum SyncStatus: Decodable, Hashable {
    case connecting, online, offline
    case error(String)

    init(from decoder: Decoder) throws {
        if let name = try? decoder.singleValueContainer().decode(String.self) {
            switch name {
            case "Online": self = .online
            case "Offline": self = .offline
            default: self = .connecting
            }
        } else {
            let error = try decoder.singleValueContainer().decode([String: String].self)
            self = .error(error["Error"] ?? "")
        }
    }
}

struct RoomEntry: Decodable, Hashable, Identifiable {
    var id: String
    var account: Int
    var name: String
    var topic: String
    var isDm: Bool
    var isInvite: Bool
    var isSpace: Bool
    var unread: Int
    var mentions: Int
    /// "Default", "All", "Mentions" or "Mute".
    var notify: String
    var favourite: Bool
    var lowPriority: Bool
}

/// A line of the room list: a heading, a folded section, a room or an
/// account.
struct Line: Decodable, Hashable {
    var row: Row
    var label: String
    var indent: Bool
    var unread: Int
    var mentions: Int
    var folded: Bool
    var target: Target

    enum Row: Decodable, Hashable {
        case header(title: String)
        /// A folded section: its title and how many rooms are in it.
        case folded(title: String, rooms: Int)
        case room(index: Int)
        case account(index: Int)

        private enum Keys: String, CodingKey { case type, title, index, rooms }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: Keys.self)
            switch try c.decode(String.self, forKey: .type) {
            case "header": self = .header(title: try c.decode(String.self, forKey: .title))
            case "folded":
                self = .folded(
                    title: try c.decode(String.self, forKey: .title),
                    rooms: try c.decode(Int.self, forKey: .rooms))
            case "room": self = .room(index: try c.decode(Int.self, forKey: .index))
            default: self = .account(index: try c.decode(Int.self, forKey: .index))
            }
        }
    }

    struct Target: Decodable, Hashable {
        /// "fold" or "room".
        var type: String
        var key: String
    }
}

/// A conversation item with its text as styled blocks.
struct Message: Decodable, Hashable, Identifiable {
    var key: String
    var eventId: String
    var kind: ItemKind
    var senderId: String
    var senderName: String
    var body: String
    var tsMs: UInt64
    var isOwn: Bool
    var edited: Bool
    var canEdit: Bool
    var canDelete: Bool
    var image: Bool
    /// What was written with a picture or file; empty without.
    var caption: String
    var pending: Bool
    var failed: Bool
    var reactions: [Reaction]
    /// Who was replied to, and the start of their text.
    var reply: [String]?
    /// The id of the message this one answers ("" if none).
    var replyTo: String
    var pinned: Bool
    /// Who has read up to this message.
    var readBy: [String]
    var thread: ThreadSummary?
    var blocks: [Block]
    var senderColor: String
    /// What can be done to it, and what "copy" takes (Rust decides both,
    /// as for the terminal).
    var can: Offered
    var copy: String

    var id: String { eventId.isEmpty ? key + body : eventId }
}

/// What can be done to a message.
struct Offered: Decodable, Hashable {
    var reply: Bool
    var react: Bool
    var edit: Bool
    var delete: Bool
    var openMedia: Bool
    var openLink: Bool
    var copy: Bool
    var copyCode: Bool
    var pin: Bool
    var forward: Bool
    var thread: Bool
    var endPoll: Bool
}

/// `[replies, [name, text] or null]`.
struct ThreadSummary: Decodable, Hashable {
    var replies: Int
    var latest: [String]?

    init(from decoder: Decoder) throws {
        var c = try decoder.unkeyedContainer()
        replies = try c.decode(Int.self)
        latest = try c.decodeIfPresent([String].self)
    }
}

struct Reaction: Decodable, Hashable {
    var key: String
    var count: Int
    var mine: Bool
    /// Who else reacted so (the user is `mine`).
    var senders: [String]

    /// "you" first, then the others, as in the terminal.
    var names: [String] { (mine ? ["you"] : []) + senders }
}

/// `"Text"`, `"Media"`, … or `{"Poll": {…}}`.
enum ItemKind: Decodable, Hashable {
    case text, emote, notice, media, state, redacted, undecryptable, dateDivider, readMarker
    case poll(Poll)

    init(from decoder: Decoder) throws {
        if let name = try? decoder.singleValueContainer().decode(String.self) {
            switch name {
            case "Emote": self = .emote
            case "Notice": self = .notice
            case "Media": self = .media
            case "State": self = .state
            case "Redacted": self = .redacted
            case "Undecryptable": self = .undecryptable
            case "DateDivider": self = .dateDivider
            case "ReadMarker": self = .readMarker
            default: self = .text
            }
        } else {
            let poll = try decoder.singleValueContainer().decode([String: Poll].self)
            guard let found = poll["Poll"] else {
                throw DecodingError.dataCorrupted(DecodingError.Context(
                    codingPath: decoder.codingPath,
                    debugDescription: "a kind of item this app does not know"))
            }
            self = .poll(found)
        }
    }
}

struct Poll: Decodable, Hashable {
    var question: String
    var maxSelections: Int
    var isEnded: Bool
    var resultsVisible: Bool
    var voterCount: Int
    var answers: [PollAnswer]
}

struct PollAnswer: Decodable, Hashable {
    var id: String
    var text: String
    var voters: [String]
    var selfVoted: Bool
    var isWinner: Bool
}

/// A paragraph of styled runs, or a code block.
enum Block: Decodable, Hashable {
    case text([Run])
    case code(language: String, content: String)

    private enum Keys: String, CodingKey { case type, runs, language, content }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        if try c.decode(String.self, forKey: .type) == "code" {
            self = .code(
                language: try c.decode(String.self, forKey: .language),
                content: try c.decode(String.self, forKey: .content))
        } else {
            self = .text(try c.decode([Run].self, forKey: .runs))
        }
    }
}

struct Run: Decodable, Hashable {
    var text: String
    /// "bold", "italic", "code", "strike", "heading", "quote".
    var styles: [String]
    var link: String?
}

struct Settings: Decodable {
    var prefs: Prefs
    /// The user's own sections, in order.
    var sections: [String]
    var accountFilter: Int?
    var unreadOnly: Bool
    var theme: Palette
    var themes: [String]
}

/// The preferences, all of them: the app sends them back whole (Rust
/// fills what is missing with defaults, which would undo settings).
struct Prefs: Codable, Equatable {
    var sort: String
    var notifications: Bool
    var images: String
    var spellcheck: Bool
    var roomEvents: Bool
    var mixedList: Bool
    var spaces: Bool
    var typingNotices: Bool
    var readReceipts: Bool
    var linkPreviews: Bool
    var languagetoolUrl: String
    var languagetoolLanguages: [String]
    var languagetoolUsername: String
    var languagetoolApiKey: String
    var proxy: String
    var longMessages: Int
    /// The terminal's (looking for new releases): kept so that sending
    /// the preferences back does not reset it. Not there in older JSON.
    var updates: Bool?
}

struct SearchHit: Decodable, Hashable, Identifiable {
    var roomId: String
    var roomName: String
    var eventId: String
    var sender: String
    var tsMs: UInt64
    var text: String

    var id: String { roomId + eventId }
}

struct RoomDetails: Decodable, Equatable {
    var roomId: String
    var name: String
    var topic: String
    var alias: String
    var encrypted: Bool
    var members: [RoomMember]
    var canInvite: Bool
    var canKick: Bool
    var canRename: Bool
    var canSetTopic: Bool
}

struct RoomMember: Decodable, Hashable {
    var userId: String
    var name: String
    /// "admin", "moderator" or "".
    var role: String
    var invited: Bool
}

/// Emoji verification: `"Waiting"`, `{"Incoming": {"device": …}}`,
/// `{"Emojis": [[emoji, name], …]}`, `"Confirming"`, `"Done"`,
/// `{"Failed": "…"}`.
enum VerifyState: Decodable, Equatable {
    case incoming(device: String)
    case waiting
    case emojis([[String]])
    case confirming
    case done
    case failed(String)

    private struct Incoming: Decodable { var device: String }

    init(from decoder: Decoder) throws {
        if let name = try? decoder.singleValueContainer().decode(String.self) {
            switch name {
            case "Confirming": self = .confirming
            case "Done": self = .done
            default: self = .waiting
            }
            return
        }
        let c = try decoder.container(keyedBy: AnyKey.self)
        if let incoming = try? c.decode(Incoming.self, forKey: AnyKey("Incoming")) {
            self = .incoming(device: incoming.device)
        } else if let emojis = try? c.decode([[String]].self, forKey: AnyKey("Emojis")) {
            self = .emojis(emojis)
        } else {
            self = .failed((try? c.decode(String.self, forKey: AnyKey("Failed"))) ?? "")
        }
    }
}

/// A key by its name, as it stands in the JSON.
struct AnyKey: CodingKey {
    var stringValue: String
    var intValue: Int? { nil }
    init(_ name: String) { stringValue = name }
    init?(stringValue: String) { self.stringValue = stringValue }
    init?(intValue: Int) { nil }
}

/// A theme's colours, `#rrggbb`.
struct Palette: Decodable, Hashable {
    var name: String
    var background: String
    var fg: String
    var dim: String
    var accent: String
    var border: String
    var selectionBg: String
    var selectionFg: String
    var unread: String
    var mention: String
    var error: String
    var success: String
    var senders: [String]

    /// The mothrix theme (the logo's colours), until Rust says otherwise.
    static let fallback = Palette(
        name: "mothrix", background: "#140a1e", fg: "#efdcb8", dim: "#7c6488",
        accent: "#f2c46d", border: "#3a2448", selectionBg: "#2e1a3d", selectionFg: "#f6e6c4",
        unread: "#ef8d27", mention: "#e8608f", error: "#e0565b", success: "#a9c98a",
        senders: ["#f2c46d"])
}

struct SignInState: Decodable, Equatable {
    var busy: Bool = false
    var lines: [String] = []
    var error: String?
}

