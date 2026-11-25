// SPDX-License-Identifier: MPL-2.0

import Foundation
import Security

/// The passphrase the accounts' stores (their encryption keys, what they
/// have read of their rooms) are encrypted with on this phone. It is made
/// once and kept in the Keychain, in the app group's access group, so
/// that the app and its two extensions read the same one. The store's
/// files alone, copied off the phone, then give nothing away.
///
/// It can be read after the first unlock since the phone started (the
/// notification extension works while the phone is locked) and never
/// leaves this device: the stores do not travel in a backup either.
enum StoreKey {
    private static let group = "group.de.tmrk.mothrix"
    private static let service = "de.tmrk.mothrix.store"
    private static let account = "passphrase"

    private static var item: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrAccessGroup as String: group,
        ]
    }

    private static func read() -> String? {
        var query = item
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var found: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &found) == errSecSuccess,
              let data = found as? Data,
              let text = String(data: data, encoding: .utf8), !text.isEmpty
        else { return nil }
        return text
    }

    /// The passphrase, made if there is none yet. Nil when the Keychain
    /// cannot be used: a store made then has none, and the sign-in says
    /// so (Rust's `sign_in_held`).
    static func passphrase() -> String? {
        if let kept = read() { return kept }
        var bytes = [UInt8](repeating: 0, count: 32)
        guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess
        else { return nil }
        let made = bytes.map { String(format: "%02x", $0) }.joined()
        var new = item
        new[kSecValueData as String] = Data(made.utf8)
        new[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        // The app and an extension can come to this at the same moment:
        // whoever was second takes what the first one kept.
        switch SecItemAdd(new as CFDictionary, nil) {
        case errSecSuccess: return read()
        case errSecDuplicateItem: return read()
        default: return nil
        }
    }

    /// Give it to Rust, before anything of mothrix is started in this
    /// process. Only the first call counts there.
    static func handOver() {
        if let passphrase = passphrase() {
            storePassphrase(passphrase: passphrase)
        }
    }
}
