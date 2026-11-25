// SPDX-License-Identifier: MPL-2.0

import Foundation

/// The number on the app's icon: the chats with something new, over all
/// accounts. The app knows it while it runs. While it does not, each push
/// says how many its account has (as its homeserver counts them), and
/// the notification extension adds the accounts up. So both keep one
/// number per account here, in the app group's settings, and show the sum.
///
/// What a push says is a server's word: a count is kept only for an
/// account that is on this phone, and within what an icon can show, so
/// that no push fills the settings with names or adds up to a number
/// that cannot be held.
enum Badge {
    private static let group = "group.de.tmrk.mothrix"
    private static let key = "badge-counts"
    /// The most one account counts for, and the most the icon shows.
    private static let most = 9_999
    private static var kept: UserDefaults? { UserDefaults(suiteName: group) }

    private static func counts() -> [String: Int] {
        kept?.dictionary(forKey: key) as? [String: Int] ?? [:]
    }

    /// Letters, digits, "-" and "_", at least one: a profile's name, as
    /// mothrix itself makes them (`clean_profile`). It becomes a folder's
    /// name, so nothing a path could be made of.
    static func isProfile(_ name: String) -> Bool {
        !name.isEmpty && name.unicodeScalars.allSatisfy {
            CharacterSet.alphanumerics.contains($0) || $0 == "-" || $0 == "_"
        }
    }

    /// Whether an account is kept on this phone under this profile: its
    /// folder in the shared one holds a session file (Rust's
    /// `signed_in_profiles`).
    static func hasAccount(_ profile: String) -> Bool {
        guard isProfile(profile),
              let root = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)
        else { return false }
        let session = root.appendingPathComponent("data", isDirectory: true)
            .appendingPathComponent(profile, isDirectory: true)
            .appendingPathComponent("session.json")
        return FileManager.default.fileExists(atPath: session.path)
    }

    /// The sum to show. Each count and the sum stay within `most`, so
    /// adding them cannot go past what a number holds.
    private static func sum(_ all: [String: Int]) -> Int {
        all.values.reduce(0) { total, count in
            min(most, total + min(most, max(0, count)))
        }
    }

    /// One account's count, as a push told it. Returns the sum to show;
    /// nil for a profile no account is kept under (nothing is kept then).
    static func heard(profile: String, unread: Int) -> Int? {
        guard hasAccount(profile) else { return nil }
        // Accounts that are gone since no longer count.
        var all = counts().filter { hasAccount($0.key) }
        all[profile] = min(most, max(0, unread))
        kept?.set(all, forKey: key)
        return sum(all)
    }

    /// Every account's count, as the running app knows them (accounts it
    /// does not name are gone). Returns the sum to show.
    static func know(_ all: [String: Int]) -> Int {
        kept?.set(all, forKey: key)
        return sum(all)
    }
}
