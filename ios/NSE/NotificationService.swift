// SPDX-License-Identifier: MPL-2.0
//
// The notification extension: iOS starts it for every push (they carry
// `mutable-content`). Pushes hold only ids (`event_id_only`); this fetches
// the message, decrypts it in the app's shared store, and shows who wrote
// what. If that fails in time, the push's own "New message" stays.

import UserNotifications

final class NotificationService: UNNotificationServiceExtension {
    private var handler: ((UNNotificationContent) -> Void)?
    private var content: UNMutableNotificationContent?

    override func didReceive(
        _ request: UNNotificationRequest,
        withContentHandler handler: @escaping (UNNotificationContent) -> Void
    ) {
        self.handler = handler
        let content = (request.content.mutableCopy() as? UNMutableNotificationContent)
            ?? UNMutableNotificationContent()
        self.content = content
        let info = request.content.userInfo
        let unread = (info["unread_count"] as? NSNumber)?.intValue
        guard let roomId = info["room_id"] as? String,
              let eventId = info["event_id"] as? String
        else {
            // No message: the homeserver tells a new count (something was
            // read elsewhere). Only the app icon's number changes.
            let quiet = UNMutableNotificationContent()
            if let unread { quiet.badge = NSNumber(value: unread) }
            self.content = quiet
            return finish()
        }
        let profile = info["mothrix_profile"] as? String ?? "default"
        if let unread {
            content.badge = NSNumber(value: unread)
        }
        // Reply and Mark as read (the app registers them).
        content.categoryIdentifier = "message"
        let folder = FileManager.default
            .containerURL(forSecurityApplicationGroupIdentifier: "group.de.tmrk.mothrix")?.path ?? ""
        // `notification` blocks while it fetches and decrypts.
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                if let found = try notification(
                    folder: folder, profile: profile, roomId: roomId, eventId: eventId)
                {
                    content.title = found.title
                    content.subtitle = found.subtitle
                    if !found.body.isEmpty { content.body = found.body }
                    content.threadIdentifier = found.room
                    content.userInfo["mothrix_room"] = found.room
                    content.sound = found.noisy ? .default : nil
                }
            } catch {
                // "New message" stays.
            }
            self.finish()
        }
    }

    /// iOS ends the extension soon: show what there is.
    override func serviceExtensionTimeWillExpire() {
        finish()
    }

    private func finish() {
        guard let handler, let content else { return }
        self.handler = nil
        handler(content)
    }
}
