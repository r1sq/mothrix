# Working on mothrix

mothrix is a terminal Matrix client (Rust, ratatui, matrix-sdk 0.19).
What it does for a user is in
[README.md](README.md); this file is what you need to change it.

## Commands

```sh
cargo build                 # target/debug/mothrix
cargo test                  # unit tests, including screen rendering and clicks
just test                   # the tests of all crates
just lint                   # cargo fmt --check and clippy (all crates) with warnings as errors
just audit                  # the crates in Cargo.lock against known advisories (asks osv.dev)
cargo build --release       # what the user runs: target/release/mothrix
just deb                    # target/mothrix_<version>_<arch>.deb
```

Before a commit: `cargo fmt`, `cargo clippy --workspace --all-targets`
without warnings, `cargo test --workspace` green. When a change is
user-visible, update the docs (`docs/features.md`, and `docs/usage.md` or
`docs/configuration.md` for keys, commands and settings; README.md only
for what belongs among its highlights), the help text in
`crates/mothrix-tui/src/main.rs` (`HELP`) and, for commands and options,
`docs/mothrix.1`.

`cargo` is in `~/.cargo/bin`; a shell that says "cargo: command not found"
needs that on its `PATH`. A piped command hides that error, so check it
once.

Disk space on the development machine is tight: a second cargo target
directory (cross builds, musl, LTO) takes over 1 GB. Check `df -h / /tmp`
first and delete such folders afterwards.

## Layout

A Cargo workspace of three crates. `mothrix-core` is everything that is not
the terminal: the Matrix backend, several accounts at once, the plain data the
UI shows, and what reads messages for showing them. Nothing in it draws
or reads keys (except the password prompt of `mothrix login`).
`mothrix-tui` is the terminal client; its binary is `mothrix`. `mothrix-ffi`
is mothrix-core for the iOS app (UniFFI; see [docs/ios.md](docs/ios.md)):
its `view.rs` keeps what the app shows and turns the app's actions into
commands, so that the SwiftUI side only draws.

The backend sends plain data ([`Update`]) and receives [`Command`]s over
channels. All state and input handling of the TUI is free of matrix-sdk,
so that it can be tested without a server. Its root re-exports the core's
modules (`use mothrix_core::{matrix, model, …}` in `main.rs`), so inside
the TUI they are `crate::matrix` and so on, like its own.

`crates/mothrix-core/src/`:

| File | What is in it |
|------|---------------|
| `accounts.rs` | several accounts at once: merges their updates, routes commands; restoring, signing in (`SignIn::from_form`, `start_sign_in`) and signing out; `Frontend` is what it needs of a UI (its `ClientState`, `on_update`, two small hooks) |
| `client_state.rs` | `ClientState`: what both front ends know about the accounts and rooms (each holds one as `state`), and what is decided on it alike: when to notify, which account a command goes to, swapping and signing out accounts, opening rooms and threads (`Opening`), closing, what an update changes of the conversation shown (`take`, `Taken`), goto, what the config file keeps (`shown`, `config_dirty`) |
| `matrix.rs` | the backend of one account: login, sync, timelines, every `Command` |
| `convert.rs` | timeline items of the SDK to `model::Item` |
| `room_watch.rs` | decisions about the open room: read receipts, polls to read, pictures to fetch |
| `stable_polls.rs` | polls in the event formats the SDK does not tally |
| `verify.rs` | emoji verification, with own devices and with other people |
| `model.rs` | the plain data the UI shows |
| `message.rs` | the rules about one message, alike in both front ends: what can be done to it (`offered`), what copy takes, which answers a vote sends, what a new poll needs, which mentions go out |
| `prefs.rs` | `Prefs`: the preferences, with their defaults |
| `config.rs` | the config file; `Shown` is what of the UI it keeps |
| `theme.rs` | the colour themes, built in and own (`[[themes]]`), as plain `#rrggbb` colours; a sender's colour |
| `sidebar.rs` | the room list: sections, order, folding, filters, its lines (`Sidebar::lines`); `Folds`, the fold state |
| `inline.rs`, `codeblock.rs` | Markdown as shown in messages |
| `preview.rs` | pictures made small for showing |
| `emoji.rs` | finding emoji by name, all of them by group, and `Usage`: the ones the user takes most (`emoji.json`) |
| `push.rs` | pushers: where the homeserver sends push notifications for phones (`mothrix pushers`, the iOS app) |
| `store_key.rs` | the passphrase of an account's store, in the system keyring (`session.json` says `"store_key": "keyring"`) |
| `folders.rs` | where mothrix keeps its files; an app (iOS) names one folder for all (`set_root`) |
| `toast.rs` | notifications on Windows (toasts) for both front ends, and registering mothrix's app id for them |
| `art.rs` | the text moth (the start screen when pictures are off or there is no room), and which part of it takes which colour |
| `sections.rs` | own sections: `OwnSections`, every change to them and their rules; in each account's account data (`de.tmrk.mothrix.sections`): an account's share, merging, `Sync` (used by `Accounts`) |
| `update.rs` | updates of the terminal client from GitHub releases: looking, checking the signed `release.json` against the built-in key (`release_key.pub`), downloading, putting the new program in place; `Build` says what a build may do |
| `search.rs`, `spell.rs`, `languagetool.rs` | what their names say |

`crates/mothrix-tui/src/`:

| File | What is in it |
|------|---------------|
| `main.rs` | arguments, state files, the main loop, snapshot mode |
| `notify.rs` | desktop notifications, and clicks on them in kitty and on Windows |
| `config.rs` | the config file, and how it maps onto `App` and back |
| `app.rs` | `App`: state, keys, mouse, slash commands |
| `app/sidebar.rs` | the room list: rows, lines ready to draw, folding, the cursor |
| `app/overlay.rs` | which dialogs are open: the stack, keys and pasted text to the top one |
| `app/settings.rs` | the settings dialog: one `Setting` per row |
| `app/suggestions.rs` | the popup above the message box (commands, files, @, emoji, spelling) |
| `app/emoji_picker.rs` | the emoji picker dialog: search, the grid by group, reacting or putting one in |
| `app/actions.rs` | what can be done to the selected message, with keys and labels |
| `app/format_menu.rs` | the format menu (`F6`, `+ format`), and double-clicks opening files and pictures (tests) |
| `app/help.rs` | the help dialog (`F1`, `?`, `/help`): the rebindable shortcuts and slash commands from their tables, the rest written there |
| `app/dialogs.rs` | search, spelling, sections, the room menu |
| `app/effects.rs` | `Effect`: what `App` asks of the world outside |
| `app/viewport.rs` | which lines of the conversation are in view |
| `app/accounts.rs` | the accounts dialog, and `App` as the accounts' `Frontend` |
| `app/roominfo.rs` | the room info dialog |
| `app/updates.rs` | new releases as the app sees them: telling once, `/update`, the settings row (the main loop looks, installs and restarts) |
| `accounts_tests.rs` | `Accounts` serving `App`, against stand-in backends |
| `ui.rs` | drawing; records the clickable regions (`app.hits`) while drawing |
| `ui/conversation.rs` | the conversation's layout: messages as rows at a width, with click targets and picture rows (`Layout::of`, tested without a screen); `can_expand` |
| `input.rs` | the text field |
| `format.rs` | Markdown formats for the message box, at the cursor (the format menu) |
| `keys.rs` | key bindings, built in and from the config |
| `theme.rs` | the core's themes (and the config's own) in ratatui colours |
| `pictures.rs` | `Pictures`: the pictures on the terminal, which way they are drawn (`Graphics`), their rows in a layout (`drawn`), what to write to the terminal for them (kitty copies, the Sixel repaint) |
| `kitty.rs`, `sixel.rs` | the two picture protocols' escape sequences |
| `logo.rs` | the logo on the start screen, as a picture under the key `mothrix:logo`; its edge fades (dithered for sixel and blocks) |

`crates/mothrix-ffi/` (the iOS app's Rust side):

| File | What is in it |
|------|---------------|
| `src/lib.rs` | the UniFFI interface: `Mothrix` (start, `act` with JSON, a `Listener` for JSON events), `notification` for the extension, `respond` (a notification's actions) and `share` (the share extension), each with its own short-lived client; the accounts' task and the `Effect`s carried out |
| `src/view.rs` | `View`: what the app shows; the accounts' `Frontend`; `Action` in, `Event` out (JSON, `{"type": …}`) |
| `src/rich.rs` | a message's Markdown as runs and code blocks (the terminal's parser) |
| `uniffi-bindgen.rs` | writes the Swift bindings: `cargo run -p mothrix-ffi --bin uniffi-bindgen -- generate --library target/debug/libmothrix_ffi.so --language swift --out-dir DIR` |

`ios/` (the iPhone app; built on the Mac, see [docs/ios.md](docs/ios.md)):

| File | What is in it |
|------|---------------|
| `build-rust.sh` | mothrix-ffi for iPhone and simulator as `build/MothrixFFI.xcframework`, the bindings in `Generated/` |
| `project.yml` | the Xcode project (XcodeGen): the app, the notification and share extensions, the tests |
| `App/Models.swift` | Rust's JSON events as Swift values |
| `App/Store.swift` | starts mothrix-ffi, keeps what the events say, sends actions; push token to pushers |
| `App/Screens.swift`, `App/Look.swift` | sign-in, room list, conversation; theme colours, monospace, the moth |
| `App/Capture.swift` | the camera, recording voice messages |
| `Share/ShareViewController.swift` | the share extension: rooms from `share-rooms.json` (the app writes it), sends through Rust's `share` |
| `NSE/NotificationService.swift` | fetches and decrypts the message a push is about |
| `MothrixTests/` | decoding the JSON samples `mothrix-ffi` writes (`MOTHRIX_WRITE_FIXTURES=1 cargo test -p mothrix-ffi`) |

`.forgejo/workflows/ios.yml` builds and tests it on the Mac's runner; by
hand, with "testflight", it uploads to TestFlight.

How a feature usually goes in: a `Command` and/or `Update` variant in
`matrix.rs` (core) and its handling there; state and keys in `app.rs` (or a file
under `app/`); drawing in `ui.rs` with a `HitAction` for the mouse; a test
next to the logic and one in `ui.rs` that renders the screen and clicks.

## Things that are not obvious

- **Room keys.** Inside the UI a room is known by a key: its id, with
  `profile/` in front for every profile but `default` (see `accounts.rs`).
  The config's sections and the drafts hold these keys. Never hand a key
  to the SDK; `Accounts::send` turns it back into an id. Which commands
  and updates carry a room is `Command::room_id_mut` and
  `Update::room_ids_mut`; both list every variant, so a new one has to
  say. `Accounts` tests run against stand-in backends (a pair of
  channels, see `accounts_tests.rs`), no server needed.
- **Clicks.** `ui.rs` pushes `Hit`s while drawing; later ones win.
  `ui::draw_overlays` covers the screen with a "close" (or "blocked") hit
  before each dialog; the dialog then pushes its own targets.
- **Dialogs** are `Overlay` variants on a stack (`App::overlays`, in
  `app/overlay.rs`); the top one gets every key, paste and click. A new
  dialog is a new variant: the compiler then asks for its keys, paste and
  drawing. Reach its state with `app.overlays.get_mut::<T>()`.
- **A new key** goes into the help too (`app/help.rs`): rebindable
  shortcuts (`keys::ACTIONS`) and slash commands (`COMMANDS`) appear by
  themselves, with a test that each has its text; the room list,
  conversation, message box, emoji picker and mouse are tables written
  there, and a test checks every message action is in them.
- **A new setting** is a field of `Prefs` with its default (`prefs.rs`;
  `app.state.prefs` holds them, the config file stores them as they are) and
  a `Setting` variant for its row (`app/settings.rs`; the compiler asks
  for its label, value and change). Snapshot mode never saves the config.
- **Effects.** Everything `App` wants done outside (commands that came
  up outside a key press, terminal sequences, notifications, clipboard,
  saving, quitting) is an `Effect` and leaves through `take_effects()`,
  in order. `on_event` returns only the commands the event itself
  caused; many tests match those exactly. `run_tui` and
  `snapshot_effects` in `main.rs` each carry effects out with one
  exhaustive match.
- **Notifications** in kitty use its OSC 99 protocol (`notify.rs`). The
  terminal's answer to a click comes in as keys (Alt+`]` …); the
  `ClickReader` takes it out of the input before `App` sees it.
- **One account, one process.** The SDK's store (and its encryption
  keys) must not be opened by two programs at once. `matrix::restore`
  and signing in take a lock on the profile's folder (`hold`), so a
  second mothrix leaves that account out and says why. The iOS app and its
  notification extension do share a store: `matrix::share_stores` turns
  that lock off and the SDK's cross-process locks on.
- **Other people's text on the screen.** Names, messages and topics go
  into ratatui as they are; `ui::scrub` takes control characters and
  bidi overrides out of the finished screen, so no drawing code has to.
  Anything written to the terminal past ratatui (`notify.rs`, pictures)
  has to encode what it sends itself.
- **Drawing goes through `ui::put`,** never `frame.render_widget`:
  ratatui panics on a rect outside the screen, and a dialog laid out for
  more rows than a low or just shrunk terminal has must only lose them.
  `ui::line_at` gives a dialog's row, or nothing where it has none.
- **Private files.** What holds tokens, drafts or decrypted attachments
  is written with `folders::write_private` (owner-only, whole or not at
  all) into a `folders::private_dir`. Deleting an account's files is
  `matrix::forget`, which leaves the lock file: the lock is on that
  file. `Config::read` tells a missing config from a broken one; who
  saves the config or needs its proxy uses it, never `load`.
- **Where a command waits.** `Session::handle` (`matrix.rs`) lists every
  `Command` and says which of three ways it goes: what uses or changes
  the conversation followed is done in the backend's loop, in order;
  what changes something on the server for a room by its id or for the
  account (invite, rename, join, key backup, forwarding into a room)
  goes to `account_lane`, in order among themselves, so that a slow
  server holds up only these; lookups get a task each. A new command
  has to be put into one of the three there.
- **Two sync loops.** The SDK's `SyncService` runs the room list and
  encryption on separate connections. Answers the crypto layer queues
  after a message that arrived in a room (verification with another
  person) wait for the encryption loop; `verify::nudge` wakes it. A room
  message can also arrive before its key: event handlers do not see it
  again once it decrypts later.
- **Fold state** of sidebar sections is kept per account when there are
  several. Only `Folds` (core `sidebar.rs`) knows how it is keyed: a click
  hands back the `action` of a `SidebarLine`, and renaming or removing an
  own section tells `Folds`; never build a fold key elsewhere.
- **Own sections change only through `OwnSections`** (names, subsections,
  tidying), in both front ends and in the sync's merge; the dialogs and
  `View::act` only show what it says.
- **Own sections travel with the account.** The UI keeps one list over
  all accounts (room keys); `Accounts` holds `sections::Sync`, merges
  what an account stores (`Update::Sections`, read at the start and
  watched in the store) into it, and `Accounts::sync_sections` (called
  in every round of the UIs' loops) sends an account its changed share
  (`Command::SetSections`). Nothing is sent before an account has said
  what it stores; echoes of older writes are ignored. Snapshot mode
  only reads. The config file keeps a copy; fold state stays per
  device.
- **Unread counts** are the larger of the server's and the SDK's own
  (it counts from the events it saw, and misses some in rooms it does
  not follow; the server cannot see mentions in encrypted messages),
  plus the "marked unread" flag;
  an open room sends a threads-included read receipt and clears the flag.
- **Polls** exist in three event formats; mothrix keeps its own tally for
  all of them and merges it with the SDK's (`convert::merge_votes`).
- **Updates** (`docs/updates.md`): only a clean release build looks
  and installs; one built with `MOTHRIX_PACKAGED` set (the `.deb` via
  `just deb`, the PKGBUILD, the release workflow's `.deb`) only tells; a
  build whose version names a commit never looks. Without a public key
  in `crates/mothrix-core/src/release_key.pub` (empty until
  `cargo run -p mothrix-release -- keygen FILE`) nothing looks either.
  `dev/release` signs releases and depends on nothing of mothrix, so the
  signing job builds it quickly. After installing, Linux `exec`s the new
  program once the terminal is restored (the account locks go with the
  old process); Windows cannot, and says to start it again.
- **Versions** are SemVer, in one place: `[workspace.package] version`
  in `Cargo.toml` (the iOS app takes it from there, `build-rust.sh`). A
  release raises it and is tagged `vX.Y.Z`. `crates/build-version.rs`
  (the build scripts of both front ends) makes `MOTHRIX_VERSION`: a clean
  build at the release's tag shows "0.2.0", any other build adds its
  commit, "0.2.0 (ce613f6)", "-dirty" with uncommitted changes. The
  terminal shows it in `--version` and the settings dialog. On GitHub a
  pushed tag builds, signs and (once approved) publishes the release
  files (`.github/workflows/release.yml`); Forgejo runs only
  `.forgejo/workflows`.
- `cargo fmt` rewraps code. A scripted edit that looks for exact text
  finds nothing after it; look at the file as it is now.

## Trying it against a server

`dev/server.sh` starts a throwaway homeserver in docker (port 16167),
fills it and signs the user `tester` in. Everything goes to `target/dev`.

```sh
dev/server.sh                      # start fresh; "dev/server.sh stop" ends it
. dev/env.sh                       # this shell's mothrix now uses target/dev
target/debug/mothrix                 # the real thing, by hand

python3 -I dev/seed-thread.py target/dev/seed.json     # a thread with replies
python3 -I dev/seed-space.py target/dev/seed.json      # a space with a subspace
python3 -I dev/seed-more.py target/dev/seed.json       # an IServ-style poll vote, Markdown, more rooms
python3 -I dev/seed-presence.py target/dev/seed.json 4 # read receipts, then typing after 4 s
```

`MOTHRIX_DEV_BIND=<this machine's LAN address> dev/server.sh` makes it
reachable on the LAN, for the iOS simulator on a Mac (`dev/mac.sh`,
`dev/sim.sh`; where the Mac is goes in `dev/local.env`, see
docs/ios.md); sign in there to `http://<that address>:16167`.

The other people are amy, ben, cleo, dan and eve, password
`pw-<name>-12345`. A second account for the multi-account views:
`MOTHRIX_PASSWORD=pw-amy-12345 target/debug/mothrix login http://127.0.0.1:16167 --user amy --profile work`.
For two mothrix instances talking to each other (verification), give the
second one its own `XDG_*_HOME` folders.

### Without a person at the keyboard

Snapshot mode syncs for some seconds, optionally presses keys, and prints
the screen as text:

```sh
target/debug/mothrix --snapshot 8 --room lunch --size 100x20
target/debug/mothrix --snapshot 14 --room lunch --say "hello"            # sends a message
target/debug/mothrix --snapshot 14 --room "work/>admin"                  # opens one room, then the next
target/debug/mothrix --snapshot 16 --room lunch \
  --say "keys:/info <enter> <wait> <down> x <wait> y <wait>"           # presses keys
```

In a key script, words are typed and names in angle brackets are keys:
`<enter> <tab> <esc> <up> <down> <space> <f1> <f2> <f3> <f4> <f5> <f6> <c-f>`; `<wait>` pauses
two seconds. Keys start two seconds after the room opened, one every 150
ms. `--room` matches a room's name or key. Leave enough `<wait>`s and
seconds for the server to answer: a result that is "not there" is often
just not there yet. The status line (last line) shows errors.

Colours and pictures do not show in text. `dev/kitty-shot.sh` runs
mothrix in kitty on a virtual screen and takes screenshots into
`target/dev/shots` (sharp pictures, colour emoji; the README's pictures
come from there, with `dev/seed-showcase.py`'s messages);
`dev/xterm-shot.sh` does the same in xterm, which draws Sixel (set
`images = "sixel"` in `target/dev/config/mothrix/config.toml`).

What a real server sends that the test server does not: OAuth-only login,
link previews, separate unread counts for threads. Say so when something
could only be tested here.

## Open

- **mothrix-ios**: see [docs/ios.md](docs/ios.md) ("State"). Not yet:
  SSO sign-in in the app, link previews (unseen), threads' first replies
  (the SDK loads only the latest; `[older]` fetches more). Untried on a
  phone: the camera, a real microphone, the share extension.
- **OAuth login** (servers without password or SSO login, as matrix.org is
  becoming): not built. The SDK needs the client to register with a public
  web address of the project: that is https://github.com/r1sq/mothrix
  (Cargo.toml's `repository`) once it is published. Needs an account on
  such a server to test with.
- **Binary size**: 96 MB (Linux, stripped). `lto = "thin"` makes it
  larger (99 MB; measured 2026-11-20, 7 min with 4 jobs, 1.7 GB of
  target). Untried: `lto = "fat"` with `codegen-units = 1`, which needs
  more memory to link than the development machine has to spare.
- **macOS and Windows**: fallbacks are in (clipboard, notifications,
  opening files). Runs on Windows 11, cross-compiled
  (`--target x86_64-pc-windows-gnu`, MinGW); its toasts, and bringing
  Windows Terminal to the front on a click
  (`notify::bring_terminal_to_front`), were never seen. Never run on
  macOS.
- **Audit of 2026-11-24**: what is left of it is in the Forgejo issues
  labelled `audit`. Changed then and never seen for real: links and
  attachments opened on Windows (`rundll32 url.dll,FileProtocolHandler`),
  the store's passphrase in the Keychain and the Credential Manager (seen
  with gnome-keyring only), a notification's Reply in a running iPhone
  app, the iPhone's file protection.
- **Arch package** (`packaging/arch/PKGBUILD`): written, never built.
- Never seen against a real server: link previews, audio through `mpv`,
  the IServ poll-vote fix, the thread part of the unread fix, signing in
  through the browser from the accounts dialog, forwarding a file into a
  room of an account on another server (the file is not uploaded again:
  the readers' servers fetch it from the first one), the refusal to
  forward an encrypted file into an unencrypted room, an update installed
  for real (looking, downloading, the restart; needs a second release on
  GitHub).
- Notifications in kitty (OSC 99) were only tried in a pseudo-terminal
  that answered like kitty, not in kitty itself (its pictures were); inside tmux or screen
  they do not get through. Elsewhere a click on a notification does
  nothing (`notify-send` could report it with `--action --wait`).
- Not there: room settings beyond name and topic, sharp pictures inside
  terminal multiplexers, a picture viewer inside the terminal, spaces more
  than one level deep, unread markers per thread.
- Verification with another person shows their request only when it
  decrypts on arrival; into a brand-new direct chat it can be missed.
- `/reset-identity` with a wrong password leaves key backup off until it
  is run again and goes through.
