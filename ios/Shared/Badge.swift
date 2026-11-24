// SPDX-License-Identifier: MPL-2.0

import Foundation

/// The number on the app's icon: the chats with something new, over all
/// accounts. The app knows it while it runs. While it does not, each push
/// says how many its account has (as its homeserver counts them), and
/// the notification extension adds the accounts up. So both keep one
/// number per account here, in the app group's settings, and show the sum.
enum Badge {
    private static let key = "badge-counts"
    private static var kept: UserDefaults? { UserDefaults(suiteName: "group.de.tmrk.mothrix") }

    private static func counts() -> [String: Int] {
        kept?.dictionary(forKey: key) as? [String: Int] ?? [:]
    }

    /// One account's count, as a push told it. Returns the sum to show.
    static func heard(profile: String, unread: Int) -> Int {
        var all = counts()
        all[profile] = max(0, unread)
        kept?.set(all, forKey: key)
        return all.values.reduce(0, +)
    }

    /// Every account's count, as the running app knows them (accounts it
    /// does not name are gone). Returns the sum to show.
    static func know(_ all: [String: Int]) -> Int {
        kept?.set(all, forKey: key)
        return all.values.reduce(0, +)
    }
}
