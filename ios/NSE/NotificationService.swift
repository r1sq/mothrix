// SPDX-License-Identifier: MPL-2.0
//
// The notification extension: iOS starts it for every push (they carry
// `mutable-content`). Pushes hold only ids (`event_id_only`); this fetches
// the message, decrypts it in the app's shared store, and shows who wrote
// what. If that fails or takes too long, it says "New message" and no more.

import Foundation
import UserNotifications

final class NotificationService: UNNotificationServiceExtension {
    /// Guards `handler` and `content`: the worker below and iOS's "time
    /// is up" come on different threads.
    private let lock = NSLock()
    private var handler: ((UNNotificationContent) -> Void)?
    private var content: UNMutableNotificationContent?

    override func didReceive(
        _ request: UNNotificationRequest,
        withContentHandler handler: @escaping (UNNotificationContent) -> Void
    ) {
        let content = (request.content.mutableCopy() as? UNMutableNotificationContent)
            ?? UNMutableNotificationContent()
        let info = request.content.userInfo
        let unread = (info["unread_count"] as? NSNumber)?.intValue
        guard let roomId = info["room_id"] as? String,
              let eventId = info["event_id"] as? String
        else {
            // No message: the homeserver tells a new count (something was
            // read elsewhere). Only the app icon's number changes.
            let quiet = UNMutableNotificationContent()
            if let unread { quiet.badge = NSNumber(value: unread) }
            begin(handler, quiet)
            return finish()
        }
        if let unread {
            content.badge = NSNumber(value: unread)
        }
        // Until the message is there, and if it never is: fixed words, not
        // the push's own (whoever sends pushes could write anything there),
        // no room for a tap or a reply to go to, and no Reply (it would
        // have nowhere to send).
        content.title = ""
        content.subtitle = ""
        content.body = "New message"
        content.categoryIdentifier = ""
        content.threadIdentifier = ""
        content.userInfo.removeValue(forKey: "mothrix_room")
        content.userInfo.removeValue(forKey: "mothrix_thread")
        begin(handler, content)
        // The profile becomes a folder's name: only what mothrix itself
        // makes of one (`clean_profile`), so no path.
        let profile = info["mothrix_profile"] as? String ?? "default"
        guard Self.isProfile(profile) else { return finish() }
        let folder = FileManager.default
            .containerURL(forSecurityApplicationGroupIdentifier: "group.de.tmrk.mothrix")?.path ?? ""
        // `notification` blocks while it fetches and decrypts.
        DispatchQueue.global(qos: .userInitiated).async {
            // Failed or nothing found: "New message" stays.
            let found = try? notification(
                folder: folder, profile: profile, roomId: roomId, eventId: eventId)
            self.finish { content in
                guard let found else { return }
                content.title = found.title
                content.subtitle = found.subtitle
                if !found.body.isEmpty { content.body = found.body }
                content.threadIdentifier = found.room
                content.userInfo["mothrix_room"] = found.room
                if let thread = found.threadRoot {
                    content.userInfo["mothrix_thread"] = thread
                }
                // Reply and Mark as read (the app registers them).
                content.categoryIdentifier = "message"
                content.sound = found.noisy ? .default : nil
            }
        }
    }

    /// iOS ends the extension soon: show what there is.
    override func serviceExtensionTimeWillExpire() {
        finish()
    }

    /// Letters, digits, "-" and "_", at least one: a profile's name.
    private static func isProfile(_ name: String) -> Bool {
        !name.isEmpty && name.unicodeScalars.allSatisfy {
            CharacterSet.alphanumerics.contains($0) || $0 == "-" || $0 == "_"
        }
    }

    private func begin(_ handler: @escaping (UNNotificationContent) -> Void, _ content: UNMutableNotificationContent) {
        lock.lock()
        self.handler = handler
        self.content = content
        lock.unlock()
    }

    /// Hand the notification to iOS, once: whoever comes first (the worker
    /// with the message, or the time running out) takes the handler, the
    /// other finds none. `change` fills the content in before, and only
    /// for the one who got the handler: nothing is written into a
    /// notification that was already handed over.
    private func finish(_ change: ((UNMutableNotificationContent) -> Void)? = nil) {
        lock.lock()
        let handler = self.handler
        let content = self.content
        self.handler = nil
        self.content = nil
        lock.unlock()
        guard let handler, let content else { return }
        change?(content)
        handler(content)
    }
}
