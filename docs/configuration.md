# Configuration

Most of it is in the settings dialog (`F2`); this is what the file holds,
and what only the file can do: your own colours and keys.

mothrix writes the file itself whenever a setting, a section or a fold
changes, and writes it whole: comments and keys it does not know are not
kept. Keep notes about your settings somewhere else.

## Where mothrix keeps its files

| | Linux | macOS | Windows |
|---|---|---|---|
| Settings | `~/.config/mothrix/config.toml` | `~/Library/Application Support/mothrix/config.toml` | `%APPDATA%\mothrix\config.toml` |
| Accounts | `~/.local/share/mothrix/<profile>/` | `~/Library/Application Support/mothrix/<profile>/` | `%APPDATA%\mothrix\<profile>\` |
| Attachments you opened | `~/.cache/mothrix/media/` | `~/Library/Caches/mothrix/media/` | `%LOCALAPPDATA%\mothrix\media\` |

Each account's folder holds its session, encryption keys and log, and
only you can enter it; the same goes for the folder of opened attachments
(they lie there decrypted) and for the files with your settings and
drafts. Attachments you opened are deleted again after a day.

An account's store (its encryption keys and what it has read of its rooms)
is encrypted with a passphrase that mothrix makes at sign-in and keeps in
the system's keyring: the desktop's Secret Service on Linux (GNOME Keyring,
KWallet), the Keychain on macOS, the Credential Manager on Windows. A copy
of the files alone (a backup, a disk taken out) then does not give the
store away. The passphrase protects the store only. The account's
`session.json`, which holds the access token that signs it in, the
drafts, the log and the attachments you opened are plain files, protected
by their permissions alone: who can read a copy of them can read these.
Where no keyring can be
reached when signing in (no desktop session, over SSH), mothrix says so
and the store is protected by the file permissions only. An account
signed in that way, or with a version before this, stays as it is until
it signs out and in again. An account whose passphrase is in the keyring
can only be started where that keyring can be read.
Next to the accounts: `emoji.json` (the emoji you take most) and
`dictionaries/`; next to the settings: `words.txt` (words the spell check
ignores).

A `config.toml` with a mistake in it (a typing error, `"yes"` where `true`
is meant) is never written over: mothrix says what is wrong, leaves the
accounts out (the file may name a proxy it must not go around) and saves
no settings until the file is put right.

## The settings

`config.toml` holds what the settings dialog changes:

- `theme`, `sort`, `images = "auto"|"kitty"|"sixel"|"blocks"|"off"` (set
  `"blocks"` if pictures show up as odd characters, e.g. inside a terminal
  multiplexer)
- switches (`true`/`false`): `notifications`, `spellcheck`, `room_events`,
  `mixed_list`, `spaces`, `typing_notices`, `read_receipts`,
  `link_previews`, `updates` (look for a new release daily; see
  [updates.md](updates.md))
- `long_messages` (lines before a message folds, `0` for never)
- `spell_languages`, `languagetool_url`, `languagetool_languages`,
  `languagetool_username`, `languagetool_api_key`
- `proxy = "http://proxy:3128"` for networks without a direct way out:
  every connection goes through it (`https://` and `socks5://`,
  `socks5h://`, `socks4://` proxies work too; `socks://`, as some
  desktops write it into `all_proxy`, is read as `socks5://`). Without it the
  environment's, as other programs read them: `HTTPS_PROXY`,
  `HTTP_PROXY` (for `http://` addresses, a LanguageTool server for one)
  and `ALL_PROXY`, none of them for the hosts `NO_PROXY` names; else
  direct (automatic proxy discovery is not used). A proxy address that
  cannot be used stops mothrix from connecting rather than going around
  it
- `account` (the profile the room list shows; empty for all),
  `[account_names]` (your names for accounts), `folded` (folded sections),
  `[[sections]]` (your sections; the accounts keep them too, and what they
  keep wins at the start)

## Your own colours

Write themes into `config.toml`, as many as you like:

```toml
theme = "goa"

[[themes]]
name = "goa"
base = "catppuccin"         # start from a built-in theme (default: mothrix)
accent = "#ff00aa"          # change what you want: fg, dim, accent, border,
unread = "#ffee00"          # selection_bg, selection_fg, unread, mention,
senders = ["#00ffcc", "#ffee00", "#ff00aa"]   # error, success; names of people
```

The built-in themes are `mothrix`, `tokyo-night`, `catppuccin`,
`gruvbox`, `nord`, `everforest` and `rose-pine`. `Ctrl+T` goes through
them all, yours included. A theme of yours needs a name of its own: one
called like a built-in theme, or like another of yours before it, is left
out, and the status bar says so at the start.

## Your own keys

A second key for a shortcut; the built-in one stays:

```toml
[keys]
search = "ctrl+s"
settings = "f9"
```

Shortcuts that can get another key: `quit`, `find_room`, `search`,
`settings`, `accounts`, `unread`, `emoji`, `format`, `theme`, `paste`,
`previous_room`, `next_room`, `switch_pane`, `scroll_up`. A key needs
Ctrl or Alt, or is an F key, so that typing stays possible, and it must
not be one that does something already: another shortcut's key, one that
edits the text (`Ctrl+A`, `Ctrl+E`, `Ctrl+U`, `Ctrl+W`), `F1` (help),
`Ctrl+C` (quit) or `Alt+Enter` (a new line). A mistake in these entries
is named in the status bar at the start.
