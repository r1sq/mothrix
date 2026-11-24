// SPDX-License-Identifier: MPL-2.0

//! The passphrase an account's store is encrypted with, kept in the
//! system's keyring (the desktop's Secret Service on Linux, the Keychain
//! on macOS, the Credential Manager on Windows). The store holds the
//! account's encryption keys and what it has read of its rooms; with the
//! passphrase elsewhere, a copy of the files alone (a backup, a disk
//! taken out) does not give them away.
//!
//! An account gets one when it signs in, if a keyring can be reached
//! then; its session file says so ([`IN_KEYRING`]). One signed in without
//! (an earlier version, a machine without a keyring, the iPhone app)
//! stays as it is until it signs in again.

use std::path::Path;

use anyhow::{Context, Result, anyhow};

/// What a session file says about a store whose passphrase is in the
/// keyring.
pub const IN_KEYRING: &str = "keyring";

/// The name all of mothrix's entries are kept under.
const SERVICE: &str = "mothrix";

/// The entry of the account kept in `data_dir`: one computer can hold
/// several sets of accounts (another user's folders, a test's), so the
/// folder names the entry, not the profile.
fn entry(data_dir: &Path) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, &data_dir.to_string_lossy()).map_err(|error| anyhow!("{error}"))
}

/// A new passphrase: 32 random bytes, as text.
fn passphrase() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| anyhow!("no random numbers: {error}"))?;
    Ok(hex::encode(bytes))
}

/// Make a passphrase for the account in `data_dir` and put it into the
/// keyring. An error: there is no keyring to reach (no desktop session,
/// say), and the store has to do without.
pub fn create(data_dir: &Path) -> Result<String> {
    let passphrase = passphrase()?;
    let entry = entry(data_dir)?;
    entry
        .set_password(&passphrase)
        .map_err(|error| anyhow!("{error}"))?;
    // Only what can be read back is worth encrypting a store with.
    let kept = entry.get_password().map_err(|error| anyhow!("{error}"))?;
    if kept != passphrase {
        anyhow::bail!("the keyring did not keep the passphrase");
    }
    Ok(passphrase)
}

/// The passphrase of the account in `data_dir`. Without it the store
/// cannot be opened; the error says what to do.
pub fn fetch(data_dir: &Path) -> Result<String> {
    entry(data_dir)
        .and_then(|entry| entry.get_password().map_err(|error| anyhow!("{error}")))
        .context(
            "this account's store is locked with a passphrase in the system keyring, which \
             cannot be read here (start mothrix in your desktop session, or sign in again)",
        )
}

/// Forget the passphrase of the account in `data_dir`, with its files.
pub fn remove(data_dir: &Path) {
    if let Ok(entry) = entry(data_dir) {
        let _ = entry.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_passphrase_is_long_random_text() {
        let (one, other) = (super::passphrase().unwrap(), super::passphrase().unwrap());
        assert_eq!(one.len(), 64);
        assert!(one.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(one, other);
    }
}
