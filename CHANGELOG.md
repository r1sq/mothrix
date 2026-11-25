# Changes

What each release brings, as its page on GitHub says it. The text under a
version is that release's notes.

## 0.3.1

The terminal client for Linux (binary and .deb, x86_64) and Windows (x86_64). The binary and the .exe update themselves (/update, or mothrix update); the .deb leaves that to your package manager. SHA256SUMS lists the files' checksums; release.json is the same list, signed. The iPhone app is on TestFlight.

**Updating from 0.2.0:** after `/update`, 0.2.0 starts itself again as 0.2.0 (a mistake in its restart). Quit and start mothrix once more and you are on 0.3.1.

### Security

A second review of the whole source, and of the first one's fixes.

- **Links are read as your browser reads them.** A link's site is the one the browser would go to, and a link whose text looks like an address shows where it really leads when the two differ. An address with a backslash or without a site is not a link and is not opened. Clicking a link after emoji or other wide characters opens that link, not its neighbour.
- **What you open is what it says it is.** An audio message that is not sound is only saved, sound is played without letting it name playlists or other files, and SVG pictures are not opened by themselves.
- **Proxies:** a mistake in the proxy setting stops the update check, dictionary downloads and LanguageTool instead of letting them go out directly. The `*_PROXY` environment variables each take what they are for, `NO_PROXY` counts, and SOCKS proxies (`socks5://`, `socks://`) work. The proxy's address no longer appears in the log.
- **Link previews** are fetched only for the room you have open, by that room's account, and never in encrypted rooms.
- **Searching all chats** no longer shows the server's hits for encrypted rooms.
- Forwarding a file without a caption no longer passes on the HTML it came with.
- `/reset-identity` opens only an `https://` page.
- A picture that would take more memory than allowed to decode is not decoded.
- A mention's link can no longer be bent by a strange user id.
- `mothrix pushers` and the other command-line modes take control characters out of what servers and people wrote.
- LanguageTool without `https://` is only allowed for addresses on this machine or your local network.
- A command typed after a space is not sent to LanguageTool and does not tell the room you are typing.

### New

- **`--snapshot`**, **`--room`**, **`--say`** and **`--size`** are in the help and the man page, as is `mothrix update`. `--say` now needs `--room`, so a message cannot land in the first room of the list.
- `config.toml` kept as a link (in your dotfiles) is written through the link, and in place where its folder takes no new files. A save that fails says so.

### Fixed

- **`/file` sends again** when another file in the folder starts with the same name (Enter took the suggestion instead), and sends a file with ` | ` in its name instead of cutting it into a path and a caption.
- **Invitations are never joined by themselves:** at a start with no key pressed, the room open last time or one a command asked for only asks. The keys for the next room pass over invitations, and the question goes away with any other key or after a minute.
- **What you write goes where you wrote it.** Messages, files, voice messages, polls and pasted pictures go only into the room and thread they were written in. When that conversation is no longer open, the text comes back into the message box instead of being sent elsewhere.
- **Nothing you type is lost:** Enter with no room open keeps the text (it goes into the room you open next), an edit keeps the draft's mentions and what it answers, and a reply begun during an edit ends the edit.
- **Verification:** the emoji wrap to the terminal's width, "They match" is offered only when all seven are on the screen, and a dialog the terminal has no room for takes only Esc and says so.
- **Polls** that ended show as ended also when reading them failed once; a reading that failed or was cut off is tried again, and votes go out in the order you made them.
- **Sections:** changes made on two devices no longer undo each other, and a signed-out account's rooms leave the sections.
- **Accounts:** a damaged session file no longer means its keys are deleted (`mothrix logout --profile NAME` removes it), an account that does not start no longer keeps its lock, and a sign-in that fails after the server took it signs the device out again.
- **Updates:** the check for a new release no longer runs into GitHub's limit of 60 questions an hour for an address, so it works behind a shared connection (a school's, a company's). Installing writes the new program under a name of its own, cleans up what a crash left, and does not wait for a new program that hangs.
- SIGINT and SIGQUIT (`kill -INT`, a script that started mothrix) end it as `q` does, with drafts saved.
- Text pasted while typing the room list's filter goes into the filter.
- A crash after a spelling suggestion went stale is gone.
- F1, Ctrl+C and Alt+Enter cannot be bound to something else by mistake.
- `/upload` is refused among the pinned messages, as `/file` is.
- Tabs in messages show as spaces; long messages with spelling marks or unclosed `*` and `` ` `` draw quickly.
- The forward dialog keeps the room you picked when the list changes.
- Looking at the pinned messages no longer marks the room read; a message with a date in the future no longer stops read receipts.
- A broken dictionary is named, and the others still check.
- The clipboard and starting a verification no longer hold up sending.
- The help lists every command.
- The `.deb`'s folders can be entered by everyone and it asks for the glibc its binary needs (2.35).

### iPhone

- A message that cannot be read no longer takes its conversation along.
- A photo, a file, a recording or a poll goes into the room and thread it was begun in, or nowhere. The share extension sends only when everything is read, and sends a file as a file.
- Ending a poll and removing a section ask first.
- An invitation's notification no longer offers Reply, and "join" joins at once.
- An account that did not start can be removed; when its store does not open it is signed out with its token alone, and the app says when the server could not be told.
- A file that is not opened is offered to the share sheet instead of staying on "fetching".
- The recovery key is copied for this device only, and for two minutes.
- "Not sent" shows while the app is open.

## 0.3.0

The terminal client for Linux (binary and .deb, x86_64) and Windows (x86_64). The binary and the .exe update themselves (/update, or mothrix update); the .deb leaves that to your package manager. SHA256SUMS lists the files' checksums; release.json is the same list, signed. The iPhone app is on TestFlight.

**Updating from 0.2.0:** after `/update`, 0.2.0 starts itself again as 0.2.0 (a mistake in its restart). Quit and start mothrix once more and you are on 0.3.0. From 0.3.0 on the restart is right.

### Security

This release follows a review of the whole source. Update if you are in rooms with people you do not know.

- **Text from other people can no longer control your terminal.** A room name, a message or a link preview could carry escape sequences, enough to write your clipboard.
- **Windows: a link in a message could run a command** when opened. Links are now handed to the desktop without going through the shell.
- **Your keys on disk are locked.** A new sign-in encrypts the account's store with a passphrase kept in your system's keyring (GNOME Keyring, KWallet, the macOS Keychain, the Windows Credential Manager). Without a keyring mothrix says so and works as before. An account signed in with an earlier version stays as it is until you sign out and in again.
- **Files are yours alone.** Account, data and cache folders can only be entered by you, and attachments you opened are deleted after a day.
- **"Open attachment" opens pictures, sound, video and documents.** Anything else (programs, scripts, web pages) is only saved, and mothrix says where. A picture is saved under the ending its content calls for, whatever its name claimed.
- **A link with a name shows where it leads:** `the menu (food.example)`.
- A forwarded message no longer carries HTML you never saw; a mention can no longer smuggle HTML from someone's display name into your message.
- A poll is ended by its author only. No link previews in encrypted rooms.
- Hostile pictures (a few kilobytes claiming to be enormous) can no longer use up your memory.
- LanguageTool addresses without `https://` are now encrypted unless they are local.

### New

- **Forwarding** to a room of another of your accounts uploads the file there, and says who wrote the message.
- **`//`** starts a message that begins with a slash; text that is no command stays in the message box instead of being thrown away.
- **Invitations ask first:** opening one says what it is, opening it again joins.
- Editing an earlier message keeps what you were writing and brings it back.
- A draft keeps who you mentioned and what you were replying to when you change rooms.
- A verification request, or a room you asked to join, no longer takes the keys of a message you are in the middle of writing.
- One verification at a time across accounts.
- `just audit` checks the dependencies against known advisories.

### Fixed

- Crashes on low or shrunk terminals, in search on a narrow terminal, and in the section manager.
- Sections no longer write back and forth between two devices; moving a section on one device moves it on the other.
- A `config.toml` with a mistake is reported and left alone, not replaced by the defaults.
- `mothrix logout` refuses while another mothrix runs with the account; a failed sign-in no longer leaves the account unlocked for a second instance.
- Kick and ban mean the person you were asked about, also when the list changed meanwhile.
- "Message" and "Verify" in a room's info use that room's account.
- Typing notices go to the right room; a slow server no longer holds up sending.
- Lines with ❤️ and similar signs are no longer cut off at their end.
- Typing no longer lays out the whole conversation again, which made long histories sluggish.
- Closing the terminal window or `kill` ends mothrix cleanly, with drafts saved.
- Updates: a new release is started once before it replaces the old one, so one that does not run on your system is never installed.
- An account added while mothrix runs no longer notifies for every unread room; a room only marked unread does not notify.
- ":D" before Enter sends the message instead of inserting an emoji (Tab still completes).

### iPhone

- A reply from a notification goes into its thread, and through the running app when it is open.
- The store is encrypted with a passphrase in the Keychain (for accounts that sign in from now on), protected until first unlock, and kept out of backups.
- The number on the app's icon follows new messages while the app is not running.
- A picked photo goes to the chat it was picked in.
- Signing out asks first.

## 0.2.0 and 0.1.0

The first releases; their pages say only what the files are.
