// SPDX-License-Identifier: MPL-2.0
//
// The screens: signing in, the room list (the terminal's sidebar, line by
// line), and a room's conversation with the message box.

import PhotosUI
import QuickLook
import UIKit
import SwiftUI

struct SignInView: View {
    @EnvironmentObject var store: Store
    @State private var server = ""
    @State private var user = ""
    @State private var password = ""

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                VStack(spacing: 4) {
                    Moth(size: 280)
                    Text("mothrix").font(.mono(.title)).foregroundStyle(store.theme.accentColor)
                }
                .frame(maxWidth: .infinity)
                field("homeserver", text: $server, hint: "matrix.example.org")
                field("user", text: $user, hint: "name")
                SecureField("", text: $password, prompt: Text("password").foregroundColor(store.theme.faint))
                    .font(.mono())
                    .padding(8)
                    .overlay(Rectangle().stroke(store.theme.line))
                    .onSubmit(go)
                WordButton(store.signIn.busy ? "[signing in …]" : "[sign in]", action: go)
                    .disabled(store.signIn.busy)
                ForEach(store.signIn.lines, id: \.self) { line in
                    Text(line).font(.mono(.footnote)).foregroundStyle(store.theme.faint)
                }
                if let error = store.signIn.error {
                    Text(error).font(.mono(.footnote)).foregroundStyle(store.theme.errorColor)
                }
                // Accounts that are signed in but did not start, and why.
                ForEach(store.problems, id: \.self) { problem in
                    Text(problem).font(.mono(.footnote)).foregroundStyle(store.theme.errorColor)
                }
            }
            .padding()
        }
    }

    private func field(_ label: String, text: Binding<String>, hint: String) -> some View {
        TextField("", text: text, prompt: Text(hint).foregroundColor(store.theme.faint))
            .font(.mono())
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .padding(8)
            .overlay(Rectangle().stroke(store.theme.line))
            .accessibilityLabel(label)
    }

    private func go() {
        store.signIn(server: server.trimmingCharacters(in: .whitespaces), user: user, password: password)
    }
}

/// The room list: headings, folded sections and rooms, as the terminal
/// lists them.
struct RoomList: View {
    @EnvironmentObject var store: Store
    @State private var showMenu = false
    @State private var searching = false
    @State private var starting = false
    @State private var naming: RoomEntry?
    @State private var newSection = ""
    @State private var leaving: RoomEntry?
    @State private var invitation: RoomEntry?

    private var roomOpen: Binding<Bool> {
        Binding(
            get: { store.openRoom != nil },
            set: { open in
                if !open { store.act("close") }
            })
    }

    var body: some View {
        NavigationStack {
            List {
                ForEach(Array(store.lines.enumerated()), id: \.offset) { _, line in
                    row(line)
                        .listRowBackground(Color.clear)
                        .listRowSeparator(.hidden)
                        .listRowInsets(EdgeInsets(top: 4, leading: 14, bottom: 4, trailing: 14))
                }
            }
            .environment(\.defaultMinListRowHeight, 24)
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            // The logo lies in the background at the bottom, faint, and the
            // rooms pass over it: it fills the room a short list leaves.
            .background(alignment: .bottom) {
                Moth(size: 320)
                    .opacity(0.35)
                    .padding(.bottom, 4)
                    .allowsHitTesting(false)
            }
            .background(store.theme.bg)
            // A header of its own, as the terminal's: no iOS toolbar (and
            // so no glass around the words).
            .toolbar(.hidden, for: .navigationBar)
            .safeAreaInset(edge: .top, spacing: 0) {
                Header { EmptyView() } title: {
                    HStack(spacing: 6) {
                        Moth(size: 30)
                        Text("mothrix")
                    }
                } trailing: { EmptyView() }
            }
            // What to do, at the bottom: in reach of the thumb.
            .safeAreaInset(edge: .bottom, spacing: 0) {
                VStack(spacing: 0) {
                    BottomBar { listFooter }
                    HStack(spacing: 12) {
                        WordButton("[+ new]") { starting = true }
                        Spacer()
                        WordButton("[find]") { searching = true }
                        Spacer()
                        WordButton("[menu]") { showMenu = true }
                    }
                    .padding(.horizontal, 16)
                    .padding(.bottom, 10)
                    .background(store.theme.bg)
                }
            }
            .sheet(isPresented: $showMenu) { MenuView().environmentObject(store) }
            .sheet(isPresented: $searching) { SearchView(everywhere: true).environmentObject(store) }
            .sheet(isPresented: $starting) { NewChatView().environmentObject(store) }
            .alert("New section", isPresented: Binding(get: { naming != nil }, set: { if !$0 { naming = nil } })) {
                TextField("name", text: $newSection)
                Button("move") {
                    if let room = naming, !newSection.trimmingCharacters(in: .whitespaces).isEmpty {
                        store.move(room.id, toSection: newSection.trimmingCharacters(in: .whitespaces))
                    }
                    newSection = ""
                }
                Button("cancel", role: .cancel) { newSection = "" }
            }
            .confirmationDialog(
                "Leave \(leaving?.name ?? "")?",
                isPresented: Binding(get: { leaving != nil }, set: { if !$0 { leaving = nil } }),
                titleVisibility: .visible
            ) {
                Button("leave", role: .destructive) { if let room = leaving { store.leave(room.id) } }
            }
            .confirmationDialog(
                "Invitation to \(invitation?.name ?? "")",
                isPresented: Binding(get: { invitation != nil }, set: { if !$0 { invitation = nil } }),
                titleVisibility: .visible
            ) {
                Button("join") { if let room = invitation { store.open(room.id) } }
                Button("decline", role: .destructive) {
                    if let room = invitation { store.act("leave", ["room": room.id]) }
                }
            }
            .toolbarBackground(store.theme.bg, for: .navigationBar)
            .navigationDestination(isPresented: roomOpen) { RoomView() }
        }
    }

    /// One line of the list, drawn as the terminal draws it: headings dim,
    /// accounts with a rule, rooms with `#` or `@`, counts at the right.
    @ViewBuilder
    private func row(_ line: Line) -> some View {
        let arrow = line.folded ? "▸" : "▾"
        let indent = line.indent ? "  " : ""
        switch line.row {
        // The line's label: what the terminal shows ("Class 7b" under
        // "School", where the section itself is "School/Class 7b").
        case .header:
            Button {
                store.fold(line.target.key)
            } label: {
                Text("\(indent)\(arrow) \(line.label)")
                    .font(.mono(.footnote).bold())
                    .foregroundStyle(store.theme.faint)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        case .folded(_, let rooms):
            Button {
                store.fold(line.target.key)
            } label: {
                HStack(spacing: 6) {
                    Text("\(indent)\(arrow) \(line.label)").bold().foregroundStyle(store.theme.faint)
                    Text("\(rooms)").foregroundStyle(store.theme.faint)
                    Spacer()
                    badge(line)
                }
                .font(.mono(.footnote))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        case .account(let index):
            Button {
                store.fold(line.target.key)
            } label: {
                HStack(spacing: 6) {
                    Text("\(arrow) \(line.label)").bold().foregroundStyle(store.theme.accentColor)
                    if index < store.accounts.count, store.accounts[index].sync != .online {
                        Text(store.accounts[index].sync == .connecting ? "○" : "●")
                            .foregroundStyle(store.theme.unreadColor)
                    }
                    Rectangle().fill(store.theme.line).frame(height: 1)
                    badge(line)
                }
                .font(.mono(.footnote))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        case .room:
            let room = store.room(line.target.key)
            let muted = room?.notify == "Mute"
            Button {
                // Opening an invited room accepts the invitation: ask first.
                if let room, room.isInvite {
                    invitation = room
                } else {
                    store.open(line.target.key)
                }
            } label: {
                HStack(spacing: 0) {
                    Text(indent + (room?.isDm == true ? "@ " : "# ")).foregroundStyle(store.theme.faint)
                    Text(line.label)
                        .lineLimit(1)
                        .fontWeight(line.unread > 0 || line.mentions > 0 ? .bold : .regular)
                        .foregroundStyle(muted ? store.theme.faint : store.theme.text)
                    Spacer(minLength: 8)
                    badge(line)
                }
                .font(.mono())
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .contextMenu {
                if let room, !room.isInvite { roomMenu(room) }
            }
            .swipeActions(edge: .leading) {
                if let room, !room.isInvite {
                    Button("read") { store.markRead(room.id) }.tint(store.theme.accentColor)
                    Button(room.favourite ? "unpin" : "pin") { store.setFavourite(room.id, !room.favourite) }
                        .tint(store.theme.unreadColor)
                }
            }
            .swipeActions(edge: .trailing) {
                if let room, !room.isInvite {
                    Button(room.notify == "Mute" ? "unmute" : "mute") {
                        store.setNotify(room.id, room.notify == "Mute" ? "Default" : "Mute")
                    }
                    .tint(store.theme.line)
                }
            }
        }
    }

    /// What is new: mentions as "@2" in the mention colour, else the
    /// count in the unread colour.
    @ViewBuilder
    private func badge(_ line: Line) -> some View {
        if line.mentions > 0 {
            Text("@\(line.mentions)").bold().foregroundStyle(store.theme.mentionColor)
        } else if line.unread > 0 {
            Text("\(line.unread)").bold().foregroundStyle(store.theme.unreadColor)
        }
    }

    /// The list's footer, as the terminal's: the accounts (when there are
    /// several), the order, and "only unread"; tapped, they change.
    private var listFooter: some View {
        HStack(spacing: 10) {
            if store.accounts.count > 1 {
                WordButton(store.accountFilter == nil ? "(all)" : "all") { store.showAccount(nil) }
                ForEach(Array(store.accounts.enumerated()), id: \.offset) { index, account in
                    WordButton(store.accountFilter == index ? "(\(account.label))" : account.label) {
                        store.showAccount(index)
                    }
                }
            }
            Spacer()
            if let prefs = store.prefs {
                WordButton("sort: \(prefs.sort)") {
                    var changed = prefs
                    switch prefs.sort {
                    case "recent": changed.sort = "name"
                    case "name": changed.sort = "unread"
                    default: changed.sort = "recent"
                    }
                    store.setPrefs(changed)
                }
            }
            Button {
                store.setUnreadOnly(!store.unreadOnly)
            } label: {
                Text(store.unreadOnly ? "● unread" : "○ unread")
                    .font(.mono(.footnote))
                    .foregroundStyle(store.unreadOnly ? store.theme.unreadColor : store.theme.faint)
            }
            .buttonStyle(.plain)
        }
    }

    /// What can be done with a room in the list.
    @ViewBuilder
    private func roomMenu(_ room: RoomEntry) -> some View {
        Button(room.favourite ? "not a favourite" : "favourite") { store.setFavourite(room.id, !room.favourite) }
        Button(room.lowPriority ? "not low priority" : "low priority") {
            store.setLowPriority(room.id, !room.lowPriority)
        }
        Menu("notifications") {
            ForEach([("Default", "as the account says"), ("All", "every message"),
                     ("Mentions", "mentions only"), ("Mute", "none")], id: \.0) { mode, label in
                Button((room.notify == mode ? "(•) " : "( ) ") + label) { store.setNotify(room.id, mode) }
            }
        }
        Menu("move to section") {
            Button("(no section)") { store.move(room.id, toSection: nil) }
            ForEach(store.sections, id: \.self) { section in
                Button(section) { store.move(room.id, toSection: section) }
            }
            Button("new section …") { naming = room }
        }
        Button("leave", role: .destructive) { leaving = room }
    }

    @ViewBuilder
    private func counts(_ line: Line) -> some View {
        if line.mentions > 0 {
            Text("@\(line.mentions)").foregroundStyle(store.theme.mentionColor)
        } else if line.unread > 0 {
            Text("\(line.unread)").foregroundStyle(store.theme.unreadColor)
        }
    }
}

struct RoomView: View {
    @EnvironmentObject var store: Store
    @State private var text = ""
    @State private var replyTo: Message?
    @State private var picked: PhotosPickerItem?
    @State private var pickingPicture = false
    @State private var pickingFile = false
    @State private var editing: Message?
    @State private var forwarding: Message?
    @State private var pickingEmoji = false
    @State private var makingPoll = false
    @State private var takingPhoto = false
    @StateObject private var recorder = VoiceRecorder()
    /// The people completed with @ in what is being written.
    @State private var mentioned: [Member] = []
    /// A draft the app itself just put into the message box: that change
    /// of the text is not the user writing.
    @State private var restored: String?
    @State private var searching = false
    @State private var showingInfo = false

    var body: some View {
        VStack(spacing: 0) {
            #if DEBUG
            Text("debug: \(store.items.count) items, thread \(store.thread ?? "-")")
                .font(.mono(.caption2)).foregroundStyle(store.theme.faint).lineLimit(1)
            #endif
            // Upside down, each message turned back: the newest message is
            // where the list starts. So a room or thread opens there, a
            // short one sits at the bottom, and new messages, pictures that
            // load and the keyboard move nothing out of sight.
            ScrollViewReader { scroller in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 6) {
                    ForEach(rows, id: \.message.id) { row in
                        MessageRow(message: row.message, showHeader: row.header, replyTo: $replyTo,
                                   editing: $editing, text: $text, forwarding: $forwarding)
                            .id(row.message.id)
                    }
                    WordButton("[older]") { store.loadOlder() }
                        .frame(maxWidth: .infinity)
                        .scaleEffect(x: 1, y: -1)
                }
                .padding(.horizontal, 10)
            }
            .scaleEffect(x: 1, y: -1)
            .onChange(of: store.revealed) { _, id in
                guard let id else { return }
                withAnimation { scroller.scrollTo(id, anchor: .center) }
                Task {
                    try? await Task.sleep(for: .seconds(2))
                    if store.revealed == id { store.revealed = nil }
                }
            }
            // The system's soft blur at the scroll edges lands in the wrong
            // place on an upside-down list.
            .scrollEdgeEffectHidden(true, for: .all)
            // Nothing of it above the header line or behind the status bar.
            .clipped()
            // A fresh list for another room or thread.
            .id((store.openRoom ?? "") + "|" + (store.thread ?? ""))
            }
            if !store.typing.isEmpty {
                Text(store.typing.joined(separator: ", ") + " …")
                    .font(.mono(.caption)).foregroundStyle(store.theme.faint)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 10)
            }
            // What to do, above the message box: in reach of the thumb.
            BottomBar {
                WordButton(store.thread == nil ? "‹ back" : "‹ room", action: back)
                Spacer()
                WordButton("[find]") { searching = true }
                Spacer()
                WordButton("[info]") {
                    store.roomInfo()
                    showingInfo = true
                }
            }
            // Nothing is written among the pinned messages.
            if store.thread != "pinned" {
                composer
            }
        }
        .background(store.theme.bg)
        .sheet(isPresented: $searching) { SearchView(everywhere: false).environmentObject(store) }
        .sheet(isPresented: $pickingEmoji) {
            EmojiPicker { emoji in
                text += emoji
                store.usedEmoji(emoji)
            }
            .environmentObject(store)
        }
        .sheet(isPresented: $makingPoll) { PollView().environmentObject(store) }
        .sheet(item: $forwarding) { message in
            ForwardView(message: message).environmentObject(store)
        }
        // iOS's own viewer, full screen with "Done": pictures, PDFs, audio,
        // video; sharing and saving from there.
        .quickLookPreview($store.mediaFile)
        // Each room (and thread) keeps what was typed and not sent.
        .onAppear { restoreDraft(draftKey) }
        .onChange(of: draftKey) { old, new in
            // An edit under way is no draft of that conversation.
            store.keepDraft(old, editing == nil ? text : "")
            // What was being answered or edited, and who was mentioned,
            // belong to the conversation left.
            replyTo = nil
            editing = nil
            mentioned = []
            restoreDraft(new)
        }
        .onChange(of: text) { _, now in
            store.keepDraft(draftKey, now)
            // Others see "typing" for what is written, not for a draft
            // that was only put back.
            let written = restored != now
            restored = nil
            if written { store.typing(!now.isEmpty) }
        }
        .sheet(isPresented: $showingInfo) { RoomInfoView().environmentObject(store) }
        .fileImporter(isPresented: $pickingFile, allowedContentTypes: [.item]) { result in
            guard case .success(let url) = result else { return }
            let allowed = url.startAccessingSecurityScopedResource()
            defer { if allowed { url.stopAccessingSecurityScopedResource() } }
            if let data = try? Data(contentsOf: url) {
                store.sendFile(data, name: url.lastPathComponent, caption: text)
                text = ""
            }
        }
        .toolbar(.hidden, for: .navigationBar)
        .safeAreaInset(edge: .top, spacing: 0) {
            Header { EmptyView() } title: {
                Text(store.thread == nil ? (store.room(store.openRoom)?.name ?? "")
                     : store.thread == "pinned" ? "pinned" : "thread")
            } trailing: { EmptyView() }
        }
        // A swipe from the left edge goes back, as elsewhere on iOS.
        .simultaneousGesture(
            DragGesture(minimumDistance: 20)
                .onEnded { drag in
                    if drag.startLocation.x < 40, drag.translation.width > 80,
                       abs(drag.translation.height) < drag.translation.width / 2
                    {
                        back()
                    }
                }
        )
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: 4) {
            if !emojiSuggestions.isEmpty {
                HStack(spacing: 14) {
                    ForEach(emojiSuggestions, id: \.self) { emoji in
                        Button(emoji) { completeEmoji(emoji) }
                            .font(.system(size: 26))
                            .buttonStyle(.plain)
                    }
                    Spacer()
                }
            }
            ForEach(suggestions, id: \.userId) { member in
                Button {
                    complete(member)
                } label: {
                    HStack {
                        Text("@" + member.name)
                        Spacer()
                        Text(member.userId).lineLimit(1).foregroundStyle(store.theme.faint)
                    }
                    .font(.mono(.caption))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            if let edited = editing {
                HStack {
                    Text("editing: \(edited.body)").lineLimit(1)
                    Spacer()
                    WordButton("[cancel]") {
                        editing = nil
                        text = ""
                    }
                }
                .font(.mono(.caption)).foregroundStyle(store.theme.faint)
            }
            if let reply = replyTo {
                HStack {
                    Text("↩ \(reply.senderName): \(reply.body)").lineLimit(1)
                    Spacer()
                    WordButton("[cancel]") { replyTo = nil }
                }
                .font(.mono(.caption)).foregroundStyle(store.theme.faint)
            }
            if recorder.recording {
                VoiceBar(recorder: recorder) {
                    if let voice = recorder.finish() {
                        store.sendVoice(voice.url, durationMs: voice.durationMs, waveform: voice.waveform)
                    }
                }
            } else {
            HStack(alignment: .bottom, spacing: 8) {
                Menu {
                    Button("emoji") { pickingEmoji = true }
                    Button("picture") { pickingPicture = true }
                    if CameraPicker.available {
                        Button("take a photo") { takingPhoto = true }
                    }
                    Button("voice message") { recorder.start() }
                    Button("file") { pickingFile = true }
                    Button("poll") { makingPoll = true }
                } label: {
                    Text("[+]").font(.mono()).foregroundStyle(store.theme.accentColor)
                }
                .photosPicker(isPresented: $pickingPicture, selection: $picked, matching: .images)
                TextField("", text: $text, prompt: Text("message").foregroundColor(store.theme.faint), axis: .vertical)
                    .font(.mono())
                    .lineLimit(1...6)
                WordButton("[send]", action: send)
            }
            }
        }
        .padding(8)
        .fullScreenCover(isPresented: $takingPhoto) {
            CameraPicker { photo in
                takingPhoto = false
                // As JPEG, like a picked photo: no location goes out.
                if let jpeg = photo?.jpegData(compressionQuality: 0.85) {
                    store.sendPicture(jpeg, name: "photo.jpg", caption: text)
                    text = ""
                }
            }
            .ignoresSafeArea()
        }
        .alert("Voice message", isPresented: Binding(
            get: { recorder.problem != nil }, set: { if !$0 { recorder.problem = nil } }
        )) {
            Button("OK") { recorder.problem = nil }
        } message: {
            Text(recorder.problem ?? "")
        }
        .overlay(alignment: .top) { Rectangle().fill(store.theme.line).frame(height: 1) }
        .onChange(of: picked) { _, item in
            guard let item else { return }
            // Loading can take seconds (a photo still in iCloud): it goes to
            // the chat it was picked in, or nowhere.
            let pickedIn = store.openRoom
            Task {
                // Drawn anew as JPEG: no location or other metadata of the
                // photo goes out, and every client can show it (not HEIC).
                if let data = try? await item.loadTransferable(type: Data.self),
                   let jpeg = UIImage(data: data)?.jpegData(compressionQuality: 0.85)
                {
                    guard store.openRoom == pickedIn else {
                        store.notice = "Not sent: another chat was opened meanwhile."
                        picked = nil
                        return
                    }
                    // What is in the message box goes with it as its caption.
                    store.sendPicture(jpeg, name: "picture.jpg", caption: text)
                    text = ""
                }
                picked = nil
            }
        }
    }

    /// The word being typed, if it starts with @.
    private var mentionQuery: String? {
        guard let last = text.split(separator: " ", omittingEmptySubsequences: false).last,
              last.hasPrefix("@")
        else { return nil }
        return String(last.dropFirst()).lowercased()
    }

    /// Members whose name or id holds what follows the @.
    private var suggestions: [Member] {
        guard let query = mentionQuery else { return [] }
        // The name, or the start of the id's own part (not its server:
        // every IServ id holds "iserv.host").
        return Array(store.members.filter {
            let local = $0.userId.dropFirst().prefix { $0 != ":" }.lowercased()
            return query.isEmpty || $0.name.lowercased().contains(query) || local.hasPrefix(query)
        }.prefix(5))
    }

    /// The word being typed, if it is like ":tada" (two letters at least).
    private var emojiQuery: String? {
        guard let last = text.split(separator: " ", omittingEmptySubsequences: false).last,
              last.hasPrefix(":"), last.count >= 3, !last.dropFirst().contains(":")
        else { return nil }
        return String(last.dropFirst()).lowercased()
    }

    /// Emoji whose names start with what follows the colon.
    private var emojiSuggestions: [String] {
        guard let query = emojiQuery else { return [] }
        var found: [String] = []
        for group in store.emojiGroups {
            for entry in group.emoji where entry.count == 2 {
                if entry[1].split(whereSeparator: { $0 == " " || $0 == "_" }).contains(where: { $0.hasPrefix(query) }) {
                    found.append(entry[0])
                    if found.count == 8 { return found }
                }
            }
        }
        return found
    }

    private func completeEmoji(_ emoji: String) {
        guard let query = emojiQuery else { return }
        text = String(text.dropLast(query.count + 1)) + emoji
        store.usedEmoji(emoji)
    }

    /// Put the member's name in place of the @word being typed.
    private func complete(_ member: Member) {
        guard let query = mentionQuery else { return }
        text = String(text.dropLast(query.count + 1)) + "@" + member.name + " "
        mentioned.append(member)
    }

    /// The messages newest first (the list is upside down), each with
    /// whether it shows its sender and time: not when the one before is by
    /// the same person within five minutes, as in the terminal.
    private var rows: [(message: Message, header: Bool)] {
        let items = store.items
        let plain: (Message) -> Bool = { message in
            switch message.kind {
            case .dateDivider, .readMarker, .state: return false
            default: return true
            }
        }
        return items.indices.map { index in
            let message = items[index]
            guard index > 0, plain(message), message.reply == nil else { return (message, true) }
            let before = items[index - 1]
            let grouped = plain(before) && before.senderId == message.senderId
                && message.tsMs >= before.tsMs && message.tsMs - before.tsMs < 300_000
            return (message, !grouped)
        }.reversed()
    }

    private var draftKey: String { (store.openRoom ?? "") + "|" + (store.thread ?? "") }

    /// Put a conversation's draft into the message box.
    private func restoreDraft(_ key: String) {
        let draft = store.draft(key)
        guard draft != text else { return }
        restored = draft
        text = draft
    }

    /// Out of the thread, or out of the room to the list.
    private func back() {
        store.act(store.thread == nil ? "close" : "close_thread")
    }

    private func send() {
        let message = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !message.isEmpty else { return }
        if let edited = editing {
            store.edit(edited.eventId, message, mentions: mentioned)
        } else if let reply = replyTo {
            store.reply(to: reply.eventId, message, mentions: mentioned)
        } else {
            store.send(message, mentions: mentioned)
        }
        text = ""
        replyTo = nil
        editing = nil
        mentioned = []
        store.typing(false)
    }
}

/// One item of the conversation.
struct MessageRow: View {
    @EnvironmentObject var store: Store
    let message: Message
    var showHeader = true
    @Binding var replyTo: Message?
    @Binding var editing: Message?
    @Binding var text: String
    @Binding var forwarding: Message?
    @State private var deleting = false
    @State private var reacting = false
    @State private var expanded = false

    /// Turned back upright here, for the upside-down list: inside the
    /// context menu, so that what the menu lifts is the right way up.
    var body: some View {
        switch message.kind {
        case .dateDivider:
            Text("── \(day) ──").font(.mono(.caption)).foregroundStyle(store.theme.faint)
                .frame(maxWidth: .infinity)
                .scaleEffect(x: 1, y: -1)
        case .readMarker:
            Rectangle().fill(store.theme.unreadColor).frame(height: 1)
        case .state:
            Text(message.body).font(.mono(.caption)).foregroundStyle(store.theme.faint)
                .scaleEffect(x: 1, y: -1)
        default:
            bubble(named: false)
                .scaleEffect(x: 1, y: -1)
                .contextMenu {
                    ForEach(store.favourites.prefix(6), id: \.self) { emoji in
                        Button(emoji) { store.react(message.eventId, emoji) }
                    }
                    Button("more emoji …") { reacting = true }
                    if message.can.reply {
                        Button("reply") { replyTo = message }
                    }
                    if message.can.copy {
                        Button("copy") { UIPasteboard.general.string = message.copy }
                    }
                    if message.can.forward {
                        Button("forward …") { forwarding = message }
                    }
                    if message.can.pin {
                        Button(message.pinned ? "unpin" : "pin for everyone") { store.togglePin(message.eventId) }
                    }
                    if message.can.endPoll {
                        Button("end poll") { store.endPoll(message.eventId) }
                    }
                    if message.can.edit {
                        Button("edit") {
                            editing = message
                            replyTo = nil
                            text = message.body
                        }
                    }
                    if message.can.delete {
                        Button("delete", role: .destructive) { deleting = true }
                    }
                } preview: {
                    // Drawn anew, with who gave each reaction (as the
                    // terminal shows for the selected message).
                    bubble(named: true)
                        .padding(10)
                        .frame(width: 360, alignment: .leading)
                        .background(store.theme.bg)
                        .foregroundStyle(store.theme.text)
                        .environmentObject(store)
                }
                .confirmationDialog("Delete this message?", isPresented: $deleting, titleVisibility: .visible) {
                    Button("delete", role: .destructive) { store.delete(message.eventId) }
                }
                .sheet(isPresented: $reacting) {
                    EmojiPicker { emoji in store.react(message.eventId, emoji) }
                        .environmentObject(store)
                }
        }
    }

    /// `named`: each reaction with who gave it (the long-press preview).
    private func bubble(named: Bool) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            if showHeader {
            HStack(spacing: 6) {
                Text(message.senderName).foregroundStyle(Color(hex: message.senderColor)).bold()
                Text(time).foregroundStyle(store.theme.faint)
                if message.edited { Text("(edited)").foregroundStyle(store.theme.faint) }
                if message.pending { Text("…").foregroundStyle(store.theme.faint) }
                if message.failed { Text("not sent").foregroundStyle(store.theme.errorColor) }
            }
            .font(.mono(.caption))
            } else if message.edited || message.pending || message.failed {
                Text(message.failed ? "not sent" : message.pending ? "…" : "(edited)")
                    .font(.mono(.caption2))
                    .foregroundStyle(message.failed ? store.theme.errorColor : store.theme.faint)
            }
            if let reply = message.reply, reply.count == 2 {
                Button {
                    store.reveal(message.replyTo)
                } label: {
                    Text("↩ \(reply[0])\(reply[1].isEmpty ? "" : ": " + reply[1])")
                        .lineLimit(2).font(.mono(.caption)).foregroundStyle(store.theme.faint)
                        .multilineTextAlignment(.leading)
                }
                .buttonStyle(.plain)
            }
            content
            linkPreview
            if let thread = message.thread {
                VStack(alignment: .leading, spacing: 1) {
                    WordButton("[\(thread.replies) in thread]") {
                        store.act("open_thread", ["event_id": message.eventId])
                    }
                    if let latest = thread.latest, latest.count == 2 {
                        Text("\(latest[0]): \(latest[1])").lineLimit(1)
                            .font(.mono(.caption)).foregroundStyle(store.theme.faint)
                    }
                }
            }
            if !message.reactions.isEmpty {
                HStack(spacing: 6) {
                    ForEach(message.reactions, id: \.key) { reaction in
                        Button {
                            store.react(message.eventId, reaction.key)
                        } label: {
                            Text("\(reaction.key) \(reaction.count)")
                                .font(.mono(.caption))
                                .padding(.horizontal, 5)
                                .overlay(Rectangle().stroke(reaction.mine ? store.theme.accentColor : store.theme.line))
                        }
                        .buttonStyle(.plain)
                    }
                }
                if named {
                    ForEach(message.reactions, id: \.key) { reaction in
                        Text("\(reaction.key) " + reaction.names.joined(separator: ", "))
                            .font(.mono(.caption)).foregroundStyle(store.theme.faint)
                    }
                }
            }
            if !message.readBy.isEmpty, store.prefs?.readReceipts != false {
                Text("✓ read by " + message.readBy.joined(separator: ", "))
                    .font(.mono(.caption2)).foregroundStyle(store.theme.faint)
            }
        }
        .padding(.vertical, 2)
        // The whole width answers taps and swipes, not only the text.
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .background(store.revealed == message.eventId ? store.theme.line : .clear)
        .onAppear { if let url = firstLink { store.previewLink(url) } }
        // A double tap reacts with the favourite emoji.
        .simultaneousGesture(TapGesture(count: 2).onEnded {
            store.react(message.eventId, store.favourites.first ?? "👍")
        })
        // A swipe to the right answers it (not from the very edge: that
        // goes back to the list). A UIKit pan that only starts sideways,
        // so that scrolling stays the scroll view's.
        .gesture(SidewaysSwipe { distance in
            if distance > 70 {
                replyTo = message
                editing = nil
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
            }
        })
    }

    @ViewBuilder
    private var content: some View {
        switch message.kind {
        case .poll(let poll):
            PollContent(poll: poll, eventId: message.eventId)
        case .media where message.image:
            Button {
                store.openMedia(message.eventId)
            } label: {
                if let picture = store.pictures[message.eventId] {
                    Image(uiImage: picture).resizable().scaledToFit().frame(maxHeight: 260)
                } else {
                    Text("[picture: \(message.body)]").font(.mono()).foregroundStyle(store.theme.faint)
                }
            }
            .buttonStyle(.plain)
            .overlay { if store.fetchingMedia == message.eventId { ProgressView() } }
            if !message.caption.isEmpty { Text(message.caption).font(.mono()) }
        case .media:
            WordButton(store.fetchingMedia == message.eventId ? "[fetching \(message.body) …]" : mediaLabel) {
                store.openMedia(message.eventId)
            }
            if !message.caption.isEmpty { Text(message.caption).font(.mono()) }
        case .redacted:
            Text("(deleted)").font(.mono()).foregroundStyle(store.theme.faint)
        case .undecryptable:
            Text("(cannot be decrypted yet)").font(.mono()).foregroundStyle(store.theme.faint)
        default:
            let limit = store.prefs?.longMessages ?? 15
            let lines = message.body.split(separator: "\n", omittingEmptySubsequences: false)
            if limit > 0, lines.count > limit, !expanded {
                VStack(alignment: .leading, spacing: 3) {
                    Text(lines.prefix(limit).joined(separator: "\n")).font(.mono())
                    WordButton("[show more: \(lines.count - limit) lines]") { expanded = true }
                }
            } else {
                RichText(blocks: message.blocks, fallback: message.body)
            }
        }
    }

    /// Title and summary of the message's first link, once fetched.
    @ViewBuilder
    private var linkPreview: some View {
        if let url = firstLink, let preview = store.previews[url], !preview.title.isEmpty {
            VStack(alignment: .leading, spacing: 2) {
                Text(preview.title).bold().lineLimit(2)
                if !preview.description.isEmpty {
                    Text(preview.description).lineLimit(3).foregroundStyle(store.theme.faint)
                }
            }
            .font(.mono(.caption))
            .padding(.leading, 8)
            .overlay(alignment: .leading) { Rectangle().fill(store.theme.line).frame(width: 2) }
        }
    }

    /// "[▶ voice 0:03]" for a voice message, "[open [file] name]" else.
    private var mediaLabel: String {
        if message.body.hasPrefix("[voice]") {
            return "[▶ voice" + message.body.dropFirst("[voice]".count) + "]"
        }
        return "[open \(message.body)]"
    }

    private var firstLink: String? {
        for block in message.blocks {
            if case .text(let runs) = block, let link = runs.first(where: { $0.link != nil })?.link {
                return link
            }
        }
        return nil
    }

    /// A divider's day: its own text, else the date of its time.
    private var day: String {
        if !message.body.isEmpty { return message.body }
        let date = Date(timeIntervalSince1970: Double(message.tsMs) / 1000)
        return date.formatted(date: .complete, time: .omitted)
    }

    private var time: String {
        let date = Date(timeIntervalSince1970: Double(message.tsMs) / 1000)
        return date.formatted(date: .omitted, time: .shortened)
    }
}

/// A message's Markdown, as Rust parsed it.
struct RichText: View {
    @EnvironmentObject var store: Store
    let blocks: [Block]
    let fallback: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if blocks.isEmpty {
                Text(fallback)
            }
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                switch block {
                case .text(let runs):
                    Text(attributed(runs))
                case .code(_, let content):
                    Text(content)
                        .font(.mono(.footnote))
                        .padding(6)
                        .padding(.top, 14)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .overlay(Rectangle().stroke(store.theme.line))
                        .overlay(alignment: .topTrailing) {
                            WordButton("[copy]") { UIPasteboard.general.string = content }
                                .padding(4)
                        }
                }
            }
        }
        .font(.mono())
        // No text selection: its double tap (select a word) would take the
        // reaction's. Copying is in the message's menu; code has [copy].
    }

    private func attributed(_ runs: [Run]) -> AttributedString {
        var all = AttributedString()
        for run in runs {
            var part = AttributedString(run.text)
            var font = Font.mono()
            if run.styles.contains("bold") || run.styles.contains("heading") { font = font.bold() }
            if run.styles.contains("italic") { font = font.italic() }
            part.font = font
            if run.styles.contains("strike") { part.strikethroughStyle = .single }
            if run.styles.contains("code") { part.backgroundColor = store.theme.line }
            if run.styles.contains("quote") { part.foregroundColor = store.theme.faint }
            if let link = run.link, let url = URL(string: link) {
                part.link = url
                part.foregroundColor = store.theme.accentColor
                part.underlineStyle = .single
            }
            all += part
        }
        return all
    }
}

/// A screen's title in mothrix's font and accent colour.
struct Title: ToolbarContent {
    @EnvironmentObject var store: Store
    let text: String

    var body: some ToolbarContent {
        ToolbarItem(placement: .principal) {
            Text(text).font(.mono(.headline)).foregroundStyle(store.theme.accentColor).lineLimit(1)
        }
    }
}

/// The menu: the theme, settings, and the accounts (verifying, signing out).
struct MenuView: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    @State private var addingAccount = false
    /// The logs in one file, made when the menu opens.
    @State private var log: URL?
    @State private var recovering: Int?
    @State private var recoveryKey = ""
    @State private var renaming: String?
    @State private var newName = ""
    /// The account to sign out: asked about first.
    @State private var signingOut: Account?

    /// The room list's order: recent, then by name, then unread first.
    private func sortButton(_ prefs: Prefs) -> some View {
        Button {
            var changed = prefs
            switch prefs.sort {
            case "recent": changed.sort = "name"
            case "name": changed.sort = "unread"
            default: changed.sort = "recent"
            }
            store.setPrefs(changed)
        } label: {
            Text("sort: \(prefs.sort)")
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowBackground(store.theme.bg)
    }

    /// A switch for one of the preferences.
    private func toggle(_ label: String, _ field: WritableKeyPath<Prefs, Bool>, _ prefs: Prefs) -> some View {
        Button {
            var changed = prefs
            changed[keyPath: field].toggle()
            store.setPrefs(changed)
        } label: {
            Text((prefs[keyPath: field] ? "[x] " : "[ ] ") + label)
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowBackground(store.theme.bg)
    }

    var body: some View {
        NavigationStack {
            List {
                if let key = store.recoveryKey {
                    Section {
                        Text(key).textSelection(.enabled)
                        HStack(spacing: 16) {
                            WordButton("[copy]") { UIPasteboard.general.string = key }
                            WordButton("[done, I kept it]") { store.recoveryKey = nil }
                        }
                    } header: {
                        Text("YOUR NEW RECOVERY KEY: keep it safe").foregroundStyle(store.theme.unreadColor)
                    }
                    .listRowBackground(store.theme.bg)
                }
                Section {
                    ForEach(store.themes, id: \.self) { name in
                        Button {
                            store.act("theme", ["name": name])
                        } label: {
                            Text((name == store.theme.name ? "(•) " : "( ) ") + name)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .listRowBackground(store.theme.bg)
                    }
                } header: {
                    Text("THEME").foregroundStyle(store.theme.faint)
                }
                if let prefs = store.prefs {
                    Section {
                        toggle("notifications", \.notifications, prefs)
                        toggle("read receipts", \.readReceipts, prefs)
                        toggle("typing notices", \.typingNotices, prefs)
                        toggle("joins and leaves", \.roomEvents, prefs)
                        toggle("rooms grouped by space", \.spaces, prefs)
                        toggle("rooms and people together", \.mixedList, prefs)
                        toggle("link previews", \.linkPreviews, prefs)
                        sortButton(prefs)
                    } header: {
                        Text("SETTINGS").foregroundStyle(store.theme.faint)
                    }
                }
                Section {
                    ForEach(Array(store.accounts.enumerated()), id: \.offset) { index, account in
                        HStack {
                            VStack(alignment: .leading) {
                                Text(account.label)
                                Text(account.userId).font(.mono(.caption)).foregroundStyle(store.theme.faint)
                            }
                            Spacer()
                            VStack(alignment: .trailing, spacing: 6) {
                                Menu {
                                    Button("set up key backup") {
                                        store.act("backup", ["account": index, "renew": false])
                                    }
                                    Button("new recovery key") {
                                        store.act("backup", ["account": index, "renew": true])
                                    }
                                    Button("enter recovery key …") {
                                        recovering = index
                                        recoveryKey = ""
                                    }
                                } label: {
                                    Text("[keys]").font(.mono(.footnote)).foregroundStyle(store.theme.accentColor)
                                }
                                if account.verified == false {
                                    WordButton("[verify]") {
                                        // The verification sheet comes from the main view:
                                        // this one has to go first.
                                        dismiss()
                                        store.act("verify_device", ["account": index])
                                    }
                                }
                                WordButton("[sign out]") { signingOut = account }
                            }
                        }
                        .listRowBackground(store.theme.bg)
                    }
                    Button {
                        addingAccount = true
                    } label: {
                        Text("[+ account]").foregroundStyle(store.theme.accentColor)
                            .frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .listRowBackground(store.theme.bg)
                    // Accounts that are signed in but did not start, and why.
                    ForEach(store.problems, id: \.self) { problem in
                        Text(problem).font(.mono(.footnote)).foregroundStyle(store.theme.errorColor)
                            .listRowBackground(store.theme.bg)
                    }
                } header: {
                    Text("ACCOUNTS").foregroundStyle(store.theme.faint)
                }
                if !store.sections.isEmpty {
                    Section {
                        ForEach(Array(store.sections.enumerated()), id: \.offset) { index, name in
                            HStack(spacing: 12) {
                                Text(name)
                                Spacer()
                                if index > 0 {
                                    WordButton("[↑]") { store.act("move_section", ["name": name, "down": false]) }
                                }
                                if index < store.sections.count - 1 {
                                    WordButton("[↓]") { store.act("move_section", ["name": name, "down": true]) }
                                }
                                WordButton("[rename]") {
                                    renaming = name
                                    newName = name
                                }
                                WordButton("[remove]") { store.act("remove_section", ["name": name]) }
                            }
                            .listRowBackground(store.theme.bg)
                        }
                    } header: {
                        Text("SECTIONS").foregroundStyle(store.theme.faint)
                    }
                }
                Section {
                    if let log {
                        ShareLink(item: log) {
                            Text("[send log]").foregroundStyle(store.theme.accentColor)
                                .frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .listRowBackground(store.theme.bg)
                    }
                    Text("What mothrix noted while running (warnings and errors, no messages), for when something goes wrong.")
                        .font(.mono(.caption)).foregroundStyle(store.theme.faint)
                        .listRowBackground(store.theme.bg)
                    if let source = URL(string: mothrixRepository()) {
                        Link(destination: source) {
                            Text("[source code]").foregroundStyle(store.theme.accentColor)
                                .frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .listRowBackground(store.theme.bg)
                    }
                    Text("mothrix \(store.versionText), MPL-2.0")
                        .font(.mono(.caption)).foregroundStyle(store.theme.faint)
                        .listRowBackground(store.theme.bg)
                } header: {
                    Text("HELP").foregroundStyle(store.theme.faint)
                }
            }
            .font(.mono())
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .background(store.theme.bg)
            .foregroundStyle(store.theme.text)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                Title(text: "menu")
                ToolbarItem(placement: .topBarTrailing) { WordButton("[done]") { dismiss() } }
                    .sharedBackgroundVisibility(.hidden)
            }
            .toolbarBackground(store.theme.bg, for: .navigationBar)
            .task { log = await store.logFile() }
            .confirmationDialog(
                "Sign out \(signingOut?.label ?? "") (\(signingOut?.userId ?? ""))?",
                isPresented: Binding(get: { signingOut != nil }, set: { if !$0 { signingOut = nil } }),
                titleVisibility: .visible
            ) {
                Button("sign out", role: .destructive) {
                    // The account asked about, wherever it is in the list now.
                    if let asked = signingOut,
                       let index = store.accounts.firstIndex(where: { $0.profile == asked.profile })
                    {
                        store.act("sign_out", ["account": index])
                        dismiss()
                    }
                }
                Button("cancel", role: .cancel) {}
            } message: {
                Text("Its messages and keys go from this phone. Encrypted messages can be read again after signing in only with the recovery key or another verified device.")
            }
            .sheet(isPresented: $addingAccount) {
                AddAccountView().environmentObject(store)
            }
            .alert("Recovery key", isPresented: Binding(get: { recovering != nil }, set: { if !$0 { recovering = nil } })) {
                SecureField("the recovery key", text: $recoveryKey)
                Button("recover") {
                    if let account = recovering {
                        store.act("recover", ["account": account, "key": recoveryKey])
                    }
                    recoveryKey = ""
                }
                Button("cancel", role: .cancel) { recoveryKey = "" }
            } message: {
                Text("Older encrypted messages become readable on this device.")
            }
            .alert("Rename section", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
                TextField("name", text: $newName)
                Button("rename") {
                    if let from = renaming, !newName.trimmingCharacters(in: .whitespaces).isEmpty {
                        store.act("rename_section", ["from": from, "to": newName.trimmingCharacters(in: .whitespaces)])
                    }
                }
                Button("cancel", role: .cancel) {}
            }
        }
        .presentationBackground(store.theme.bg)
        .notices()
    }
}

/// Search messages: in the open room, or in every room.
struct SearchView: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    @State var everywhere: Bool
    @State private var text = ""
    @FocusState private var typing: Bool

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 8) {
                TextField("", text: $text, prompt: Text("search").foregroundColor(store.theme.faint))
                    .font(.mono())
                    .textInputAutocapitalization(.never)
                    .padding(8)
                    .overlay(Rectangle().stroke(store.theme.line))
                    .onSubmit { store.search(text, everywhere: everywhere) }
                    .focused($typing)
                    .onAppear { typing = true }
                    .submitLabel(.search)
                if store.openRoom != nil {
                    WordButton(everywhere ? "(•) all rooms  ( ) this room" : "( ) all rooms  (•) this room") {
                        everywhere.toggle()
                        if !text.isEmpty { store.search(text, everywhere: everywhere) }
                    }
                }
                List(store.searchQuery == text.trimmingCharacters(in: .whitespaces) ? store.searchHits : []) { hit in
                    Button {
                        store.showHit(hit)
                        dismiss()
                    } label: {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("\(hit.roomName) · \(hit.sender)").font(.mono(.caption)).foregroundStyle(store.theme.faint)
                            Text(hit.text).font(.mono()).lineLimit(3)
                        }
                    }
                    .buttonStyle(.plain)
                    .listRowBackground(store.theme.bg)
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
            }
            .padding()
            .background(store.theme.bg)
            .foregroundStyle(store.theme.text)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                Title(text: "find")
                ToolbarItem(placement: .topBarTrailing) { WordButton("[done]") { dismiss() } }
                    .sharedBackgroundVisibility(.hidden)
            }
            .toolbarBackground(store.theme.bg, for: .navigationBar)
        }
        .presentationBackground(store.theme.bg)
        .notices()
    }
}

/// The open room: name, topic, encryption, members; inviting someone.
struct RoomInfoView: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    @State private var invitee = ""
    @State private var name = ""
    @State private var topic = ""
    @State private var removing: RoomMember?

    var body: some View {
        NavigationStack {
            List {
                if let details = store.roomDetails {
                    Section {
                        if details.canRename {
                            HStack {
                                TextField("", text: $name, prompt: Text(details.name).foregroundColor(store.theme.faint))
                                WordButton("[rename]") {
                                    store.act("rename_room", ["room": details.roomId, "name": name])
                                    name = ""
                                }
                            }
                        } else {
                            Text(details.name).bold()
                        }
                        if details.canSetTopic {
                            HStack {
                                TextField("", text: $topic,
                                          prompt: Text(details.topic.isEmpty ? "topic" : details.topic)
                                            .foregroundColor(store.theme.faint))
                                WordButton("[set topic]") {
                                    store.act("set_topic", ["room": details.roomId, "topic": topic])
                                    topic = ""
                                }
                            }
                        } else if !details.topic.isEmpty {
                            Text(details.topic).foregroundStyle(store.theme.faint)
                        }
                        if !details.alias.isEmpty { Text(details.alias).foregroundStyle(store.theme.faint) }
                        Text(details.encrypted ? "encrypted" : "not encrypted")
                            .foregroundStyle(details.encrypted ? Color(hex: store.theme.success) : store.theme.errorColor)
                        Button {
                            store.act("open_pinned")
                            dismiss()
                        } label: {
                            Text("[pinned messages]").foregroundStyle(store.theme.accentColor)
                                .frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                    .listRowBackground(store.theme.bg)
                    Section {
                        ForEach(details.members, id: \.userId) { member in
                            HStack {
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(member.name + (member.role.isEmpty ? "" : "  (\(member.role))")
                                         + (member.invited ? "  invited" : ""))
                                    Text(member.userId).font(.mono(.caption2)).foregroundStyle(store.theme.faint)
                                }
                                Spacer()
                                if !store.accounts.contains(where: { $0.userId == member.userId }) {
                                    VStack(alignment: .trailing, spacing: 6) {
                                        WordButton("[verify]") {
                                            // The verification sheet comes from the main view.
                                            dismiss()
                                            store.act("verify_user", ["user_id": member.userId])
                                        }
                                        if details.canKick {
                                            WordButton("[remove]") { removing = member }
                                        }
                                    }
                                }
                            }
                            .listRowBackground(store.theme.bg)
                        }
                        if details.canInvite {
                            HStack {
                                TextField("", text: $invitee, prompt: Text("@someone:server").foregroundColor(store.theme.faint))
                                    .textInputAutocapitalization(.never)
                                    .autocorrectionDisabled()
                                WordButton("[invite]") {
                                    store.act("invite", ["room": details.roomId, "user_id": invitee.trimmingCharacters(in: .whitespaces)])
                                    invitee = ""
                                }
                            }
                            .listRowBackground(store.theme.bg)
                        }
                    } header: {
                        Text("MEMBERS (\(details.members.count))").foregroundStyle(store.theme.faint)
                    }
                } else {
                    ProgressView().listRowBackground(store.theme.bg)
                }
            }
            .font(.mono())
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .background(store.theme.bg)
            .foregroundStyle(store.theme.text)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                Title(text: "info")
                ToolbarItem(placement: .topBarTrailing) { WordButton("[done]") { dismiss() } }
                    .sharedBackgroundVisibility(.hidden)
            }
            .toolbarBackground(store.theme.bg, for: .navigationBar)
            .confirmationDialog(
                "Remove \(removing?.name ?? "") from the room?",
                isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
                titleVisibility: .visible
            ) {
                Button("remove", role: .destructive) {
                    if let member = removing, let room = store.roomDetails?.roomId {
                        store.act("kick", ["room": room, "user_id": member.userId])
                    }
                }
            }
        }
        .presentationBackground(store.theme.bg)
        .notices()
    }
}

/// Emoji verification, wherever it was started.
struct VerifyView: View {
    @EnvironmentObject var store: Store

    var body: some View {
        VStack(spacing: 18) {
            Text("verification").font(.mono(.headline)).foregroundStyle(store.theme.accentColor)
            switch store.verify {
            case .incoming(let device):
                Text("\(device) wants to verify this device.")
                WordButton("[accept]") { store.act("verify_answer", ["answer": "accept"]) }
                WordButton("[cancel]") { store.act("verify_close") }
            case .waiting, .confirming, .none:
                ProgressView().tint(store.theme.accentColor)
                Text("waiting for the other side …").foregroundStyle(store.theme.faint)
                WordButton("[cancel]") { store.act("verify_close") }
            case .emojis(let emojis):
                Text("Do they show the same emoji?")
                LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 4)) {
                    ForEach(Array(emojis.enumerated()), id: \.offset) { _, pair in
                        VStack {
                            Text(pair.first ?? "").font(.system(size: 34))
                            Text(pair.last ?? "").font(.mono(.caption2)).foregroundStyle(store.theme.faint)
                        }
                    }
                }
                HStack(spacing: 24) {
                    WordButton("[they match]") { store.act("verify_answer", ["answer": "confirm"]) }
                    WordButton("[they don't]") { store.act("verify_answer", ["answer": "cancel"]) }
                }
            case .done:
                Text("Verified.").foregroundStyle(Color(hex: store.theme.success))
                WordButton("[close]") { store.act("verify_close") }
            case .failed(let why):
                Text("Not verified: \(why)").foregroundStyle(store.theme.errorColor)
                WordButton("[close]") { store.act("verify_close") }
            }
        }
        .font(.mono())
        .padding()
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(store.theme.bg)
        .foregroundStyle(store.theme.text)
        .presentationBackground(store.theme.bg)
        .notices()
        .interactiveDismissDisabled()
    }
}

/// Every emoji: a search, the favourites first, then group by group.
struct EmojiPicker: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    let pick: (String) -> Void
    @State private var query = ""

    private let columns = [GridItem(.adaptive(minimum: 44))]

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    TextField("", text: $query, prompt: Text("find an emoji (smile, heart, cat …)").foregroundColor(store.theme.faint))
                        .font(.mono())
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .padding(8)
                        .overlay(Rectangle().stroke(store.theme.line))
                    if query.isEmpty {
                        group("USED OFTEN", store.favourites)
                        ForEach(store.emojiGroups, id: \.title) { group in
                            self.group(group.title.uppercased(), group.emoji.compactMap(\.first))
                        }
                    } else {
                        group("FOUND", found)
                    }
                }
                .padding()
            }
            .background(store.theme.bg)
            .foregroundStyle(store.theme.text)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                Title(text: "emoji")
                ToolbarItem(placement: .topBarTrailing) { WordButton("[done]") { dismiss() } }
                    .sharedBackgroundVisibility(.hidden)
            }
            .toolbarBackground(store.theme.bg, for: .navigationBar)
        }
        .presentationBackground(store.theme.bg)
        .notices()
    }

    /// Emoji whose words start with what is typed, then those holding it.
    private var found: [String] {
        let words = query.lowercased().trimmingCharacters(in: .whitespaces)
        var starting: [String] = []
        var holding: [String] = []
        for group in store.emojiGroups {
            for entry in group.emoji where entry.count == 2 {
                let names = entry[1].split(whereSeparator: { $0 == " " || $0 == "_" })
                if names.contains(where: { $0.hasPrefix(words) }) {
                    starting.append(entry[0])
                } else if entry[1].contains(words) {
                    holding.append(entry[0])
                }
            }
        }
        return Array((starting + holding).prefix(200))
    }

    private func group(_ title: String, _ emoji: [String]) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.mono(.caption)).foregroundStyle(store.theme.faint)
            LazyVGrid(columns: columns, spacing: 6) {
                ForEach(Array(emoji.enumerated()), id: \.offset) { _, glyph in
                    Button {
                        pick(glyph)
                        dismiss()
                    } label: {
                        Text(glyph).font(.system(size: 30))
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }
}

/// A poll as the terminal draws it: each answer with its marker, the
/// count and share once results show, a bar and who voted for it; the
/// state of the poll under it. A tap on an answer votes.
struct PollContent: View {
    @EnvironmentObject var store: Store
    let poll: Poll
    let eventId: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("▍ ").foregroundStyle(store.theme.accentColor) + Text(poll.question).bold()
            ForEach(poll.answers, id: \.id) { answer in
                Button {
                    store.vote(eventId, answer.id)
                } label: {
                    row(answer)
                }
                .buttonStyle(.plain)
                // Not `disabled`: that would fade the results.
                .allowsHitTesting(!poll.isEnded && !eventId.isEmpty)
            }
            Text(status).font(.mono(.caption)).foregroundStyle(store.theme.faint)
        }
        .font(.mono())
    }

    private func row(_ answer: PollAnswer) -> some View {
        let highlight = answer.isWinner ? store.theme.successColor : store.theme.accentColor
        let votes = answer.voters.count
        let share = poll.resultsVisible && poll.voterCount > 0 ? Double(votes) / Double(poll.voterCount) : 0
        return VStack(alignment: .leading, spacing: 1) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(marker(answer)).foregroundStyle(answer.selfVoted ? highlight : store.theme.faint)
                (Text(answer.isWinner ? "★ " : "").foregroundStyle(store.theme.successColor)
                    + Text(answer.text).foregroundStyle(answer.isWinner ? store.theme.successColor : store.theme.text))
                    .bold(answer.isWinner)
                Spacer(minLength: 6)
                if poll.resultsVisible {
                    Text("\(votes) · \(Int((share * 100).rounded()))%").foregroundStyle(store.theme.faint)
                }
            }
            if poll.resultsVisible || !answer.voters.isEmpty {
                VStack(alignment: .leading, spacing: 1) {
                    HStack(spacing: 0) {
                        Rectangle().fill(highlight).frame(width: 120 * share)
                        Rectangle().fill(store.theme.line).frame(width: 120 * (1 - share))
                    }
                    .frame(height: 6)
                    .padding(.vertical, 3)
                    if !answer.voters.isEmpty {
                        Text(answer.voters.joined(separator: ", "))
                            .font(.mono(.caption)).foregroundStyle(store.theme.faint)
                    }
                }
                .padding(.leading, 22)
            }
        }
        .contentShape(Rectangle())
    }

    /// Once a poll has ended only one's own answers keep a marker.
    private func marker(_ answer: PollAnswer) -> String {
        switch (answer.selfVoted, poll.maxSelections > 1) {
        case (true, false): return "●"
        case (true, true): return "■"
        case (false, _) where poll.isEnded: return " "
        case (false, false): return "○"
        case (false, true): return "□"
        }
    }

    private var status: String {
        if poll.isEnded { return "Final results · Votes: \(poll.voterCount)" }
        if eventId.isEmpty { return "Sending poll…" }
        if !poll.resultsVisible { return "Votes: \(poll.voterCount) · Results are shown when the poll ends" }
        if poll.maxSelections > 1 { return "Votes: \(poll.voterCount) · Choose up to \(poll.maxSelections) · tap an answer to vote" }
        return "Votes: \(poll.voterCount) · tap an answer to vote"
    }
}

/// A new poll: the question, two or more answers, one or several picks.
struct PollView: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    @State private var question = ""
    @State private var answers = ["", ""]
    @State private var multiple = false
    @State private var hidden = false

    private var ready: Bool {
        !question.trimmingCharacters(in: .whitespaces).isEmpty
            && answers.filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }.count >= 2
    }

    var body: some View {
        NavigationStack {
            List {
                field("question", $question)
                ForEach(answers.indices, id: \.self) { index in
                    field("answer \(index + 1)", $answers[index])
                }
                Button {
                    answers.append("")
                } label: {
                    Text("[+ answer]")
                        .foregroundStyle(store.theme.accentColor)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .listRowBackground(store.theme.bg)
                switchRow("people may pick several", $multiple)
                switchRow("results only when it ends", $hidden)
            }
            .font(.mono())
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .background(store.theme.bg)
            .foregroundStyle(store.theme.text)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                Title(text: "poll")
                ToolbarItem(placement: .topBarLeading) { WordButton("[cancel]") { dismiss() } }
                    .sharedBackgroundVisibility(.hidden)
                ToolbarItem(placement: .topBarTrailing) {
                    WordButton("[create]") {
                        store.createPoll(
                            question: question,
                            answers: answers.filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty },
                            multiple: multiple, hidden: hidden)
                        dismiss()
                    }
                    .disabled(!ready)
                    .opacity(ready ? 1 : 0.4)
                }
                .sharedBackgroundVisibility(.hidden)
            }
            .toolbarBackground(store.theme.bg, for: .navigationBar)
        }
        .presentationBackground(store.theme.bg)
        .notices()
    }

    private func field(_ hint: String, _ text: Binding<String>) -> some View {
        TextField("", text: text, prompt: Text(hint).foregroundColor(store.theme.faint))
            .padding(8)
            .overlay(Rectangle().stroke(store.theme.line))
            .listRowBackground(store.theme.bg)
            .listRowSeparator(.hidden)
    }

    private func switchRow(_ label: String, _ on: Binding<Bool>) -> some View {
        Button {
            on.wrappedValue.toggle()
        } label: {
            Text((on.wrappedValue ? "[x] " : "[ ] ") + label)
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowBackground(store.theme.bg)
    }
}

/// Another account: the sign-in form, closed once the account is there.
struct AddAccountView: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    @State private var accountsBefore = 0

    var body: some View {
        NavigationStack {
            SignInView()
                .background(store.theme.bg)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    Title(text: "add account")
                    ToolbarItem(placement: .topBarTrailing) { WordButton("[cancel]") { dismiss() } }
                    .sharedBackgroundVisibility(.hidden)
                }
                .toolbarBackground(store.theme.bg, for: .navigationBar)
        }
        .presentationBackground(store.theme.bg)
        .notices()
        .onAppear { accountsBefore = store.accounts.count }
        .onChange(of: store.accounts.count) { _, now in
            if now > accountsBefore { dismiss() }
        }
    }
}

/// A new chat with someone, a new room, or joining one by its address.
struct NewChatView: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    @State private var what = 0
    @State private var text = ""
    @State private var account = 0

    private let kinds = ["chat with", "new room", "join"]
    private let hints = ["@someone:server", "name of the room", "#room:server or !id:server"]

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 14) {
                    ForEach(kinds.indices, id: \.self) { index in
                        WordButton(what == index ? "(\(kinds[index]))" : kinds[index]) { what = index }
                    }
                }
                if store.accounts.count > 1 {
                    HStack(spacing: 14) {
                        Text("as").foregroundStyle(store.theme.faint)
                        ForEach(Array(store.accounts.enumerated()), id: \.offset) { index, item in
                            WordButton(account == index ? "(\(item.label))" : item.label) { account = index }
                        }
                    }
                }
                TextField("", text: $text, prompt: Text(hints[what]).foregroundColor(store.theme.faint))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .padding(8)
                    .overlay(Rectangle().stroke(store.theme.line))
                    .onSubmit(go)
                WordButton("[go]", action: go)
                Spacer()
            }
            .font(.mono())
            .padding()
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(store.theme.bg)
            .foregroundStyle(store.theme.text)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                Title(text: "new")
                ToolbarItem(placement: .topBarTrailing) { WordButton("[cancel]") { dismiss() } }
                    .sharedBackgroundVisibility(.hidden)
            }
            .toolbarBackground(store.theme.bg, for: .navigationBar)
        }
        .presentationBackground(store.theme.bg)
        .notices()
    }

    private func go() {
        let value = text.trimmingCharacters(in: .whitespaces)
        guard !value.isEmpty else { return }
        switch what {
        case 0: store.startChat(account: account, userId: value)
        case 1: store.createRoom(account: account, name: value)
        default: store.joinRoom(account: account, address: value)
        }
        dismiss()
    }
}

/// A screen's top line, as the terminal's title bars: words left and
/// right, the title in the middle, a rule below.
struct Header<Leading: View, Title: View, Trailing: View>: View {
    @EnvironmentObject var store: Store
    @ViewBuilder var leading: Leading
    @ViewBuilder var title: Title
    @ViewBuilder var trailing: Trailing

    var body: some View {
        VStack(spacing: 0) {
            ZStack {
                title
                    .font(.mono(.headline))
                    .foregroundStyle(store.theme.accentColor)
                    .lineLimit(1)
                    .padding(.horizontal, 20)
                HStack(spacing: 12) {
                    leading
                    Spacer()
                    trailing
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            Rectangle().fill(store.theme.line).frame(height: 1)
        }
        .background(store.theme.bg)
    }
}

/// Pick the room to send a message on to (rooms of the same account).
struct ForwardView: View {
    @EnvironmentObject var store: Store
    @Environment(\.dismiss) private var dismiss
    let message: Message
    /// The room tapped: asked about before the message goes.
    @State private var confirming: RoomEntry?

    /// What was typed to narrow the list.
    @State private var filter = ""

    /// The rooms of every account, the open room's first, narrowed by
    /// what was typed (as in the terminal: no spaces, no invitations).
    private var rooms: [RoomEntry] {
        let account = store.room(store.openRoom)?.account
        let needle = filter.trimmingCharacters(in: .whitespaces).lowercased()
        let rooms = store.rooms.filter {
            !$0.isInvite && !$0.isSpace && (needle.isEmpty || $0.name.lowercased().contains(needle))
        }
        return rooms.filter { $0.account == account } + rooms.filter { $0.account != account }
    }

    /// With several accounts, whose the room is.
    private func name(_ room: RoomEntry) -> String {
        guard store.accounts.count > 1, store.accounts.indices.contains(room.account) else { return room.name }
        return "\(room.name) · \(store.accounts[room.account].label)"
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                TextField("", text: $filter, prompt: Text("room").foregroundColor(store.theme.faint))
                    .font(.mono())
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .padding(8)
                    .overlay(Rectangle().stroke(store.theme.line))
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                List(rooms) { room in
                    Button {
                        confirming = room
                    } label: {
                        Text(name(room)).frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .listRowBackground(store.theme.bg)
                }
                .font(.mono())
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
            }
            .background(store.theme.bg)
            .foregroundStyle(store.theme.text)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                Title(text: "forward to")
                ToolbarItem(placement: .topBarTrailing) { WordButton("[cancel]") { dismiss() } }
                    .sharedBackgroundVisibility(.hidden)
            }
            .confirmationDialog(
                "Forward to " + (confirming.map { name($0) } ?? "") + "?",
                isPresented: Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } }),
                titleVisibility: .visible,
                presenting: confirming
            ) { room in
                Button("forward") {
                    store.forward(message.eventId, to: room.id)
                    dismiss()
                }
                Button("cancel", role: .cancel) {}
            }
            .toolbarBackground(store.theme.bg, for: .navigationBar)
        }
        .presentationBackground(store.theme.bg)
        .notices()
    }
}

/// A row of words at the bottom of a screen, with a rule above.
struct BottomBar<Content: View>: View {
    @EnvironmentObject var store: Store
    @ViewBuilder var content: Content

    var body: some View {
        VStack(spacing: 0) {
            Rectangle().fill(store.theme.line).frame(height: 1)
            HStack(spacing: 12) { content }
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
        }
        .background(store.theme.bg)
    }
}

/// A pan to the right that only begins when the finger moves clearly
/// sideways and not from the screen's left edge; vertical moves stay with
/// the scroll view. Reports how far it went.
struct SidewaysSwipe: UIGestureRecognizerRepresentable {
    let ended: (CGFloat) -> Void

    func makeUIGestureRecognizer(context: Context) -> UIPanGestureRecognizer {
        let pan = UIPanGestureRecognizer()
        pan.delegate = context.coordinator
        return pan
    }

    func handleUIGestureRecognizerAction(_ pan: UIPanGestureRecognizer, context: Context) {
        if pan.state == .ended {
            ended(pan.translation(in: pan.view).x)
        }
    }

    func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator { Coordinator() }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        func gestureRecognizerShouldBegin(_ recognizer: UIGestureRecognizer) -> Bool {
            guard let pan = recognizer as? UIPanGestureRecognizer else { return false }
            let speed = pan.velocity(in: pan.view)
            return speed.x > 0 && speed.x > abs(speed.y) * 2 && pan.location(in: nil).x > 40
        }
    }
}
