<p align="center">
  <img src="assets/mothrix-logo.png" alt="the mothrix moth before a ringed planet" width="220">
</p>

<h1 align="center">mothrix</h1>

<p align="center">
  A <a href="https://matrix.org">Matrix</a> client for the terminal, and for the iPhone, written in Rust.
</p>

<p align="center">
  <a href="https://github.com/r1sq/mothrix/releases/latest"><img src="https://img.shields.io/github/v/release/r1sq/mothrix?include_prereleases&label=release" alt="latest release"></a>
  <a href="https://github.com/r1sq/mothrix/actions/workflows/ci.yml"><img src="https://github.com/r1sq/mothrix/actions/workflows/ci.yml/badge.svg?branch=main" alt="ci"></a>
  <a href="https://github.com/r1sq/mothrix/actions/workflows/release.yml"><img src="https://github.com/r1sq/mothrix/actions/workflows/release.yml/badge.svg" alt="release build"></a>
  <img src="https://img.shields.io/badge/status-alpha-orange" alt="status: alpha">
  <a href="LICENSE"><img src="https://img.shields.io/github/license/r1sq/mothrix" alt="license: MPL-2.0"></a>
  <img src="https://img.shields.io/badge/rust-1.100-b7410e?logo=rust" alt="Rust 1.100">
</p>

> [!WARNING]
> **mothrix is alpha software.** It is young: expect bugs, missing pieces
> and changes between releases.
>
> - **Keep another way into your account.** Save your recovery key (`/backup`)
>   and keep a second verified client (Element, or another device), so that
>   a bug in mothrix cannot lock you out of your encrypted messages.
> - **It has not been audited independently.** The encryption itself is
>   [matrix-rust-sdk](https://github.com/matrix-org/matrix-rust-sdk)'s, but
>   the code around it is mothrix's own and has only been reviewed by its
>   makers (with AI tools, see [below](#developed-with-ai)). Think twice before
>   trusting it with conversations where a mistake would hurt.
> - **Some of it was never tried against a real server or device**: see
>   "Open" in [AGENTS.md](AGENTS.md#open) for the list. Servers that only
>   offer OAuth login are not supported yet, and macOS was never run.
> - **The iPhone app** is not in the App Store; it is tried out through TestFlight.
>
> Bugs and surprises are welcome as [issues](https://github.com/r1sq/mothrix/issues).

![mothrix in kitty: the room list, a picture, a code block, reactions](docs/screenshots/terminal.png)

mothrix is a chat client for people who live in the terminal: a room list
on the left, the conversation on the right, the mouse and the keyboard
for everything. Pictures show right in the conversation, encryption works
as in any other client, and several accounts sit side by side. The same
Rust core also drives an iPhone app.

## Highlights

- **Pictures in the terminal**: sharp in kitty, Ghostty and terminals with
  Sixel, coloured half blocks everywhere else
- **Mouse and keyboard**: click rooms, messages, poll answers and
  reactions, or do it all from the keyboard
- **End-to-end encryption**, verification by emoji, key backup and
  recovery; the keys on disk are locked with a passphrase in your
  system's keyring
- **Several accounts at once**, on one server or many
- **Your own sections** in the room list, which travel with the account
  to your other devices
- **Polls** in all three formats in use, with bars and who voted for what
- **Threads, replies, reactions, edits**, Markdown both ways, code blocks
  with a copy button
- **Spell checking** while you type, in several languages at once, and
  grammar checking through LanguageTool if you like
- **Search** in one chat or all of them, encrypted rooms included
- **Desktop notifications**; in kitty and on Windows a click opens the chat
- **Themes**, built in and your own
- **Linux and Windows**; macOS should work, but has not been tried

All of it in [docs/features.md](docs/features.md).

<p align="center">
  <img src="docs/screenshots/terminal-polls.png" alt="polls with bars, a hidden ballot, a thread" width="49%">
  <img src="docs/screenshots/iphone.png" alt="the iPhone app" width="49%">
</p>

## Getting started

```sh
cargo build --release
./target/release/mothrix            # asks for your homeserver on the first start
```

Your homeserver needs sliding sync (Synapse and Continuwuity have it).
Packages, a static build, and Windows and macOS: [docs/install.md](docs/install.md).

Once it runs: `Tab` switches between the room list and the message box,
`Ctrl+K` finds a room, `F2` opens the settings, and `/` in the message box
lists the commands. More in [docs/usage.md](docs/usage.md).

## On the iPhone

The app has the room list, conversations, pictures, voice messages,
reactions, threads and polls, push notifications decrypted on the phone,
and "mothrix" in other apps' share sheet. It is not in the App Store;
how it is built, and how to build your own, is in [docs/ios.md](docs/ios.md).

## Documentation

- [Features](docs/features.md): everything mothrix does, and what it does not do yet
- [Usage](docs/usage.md): keys, commands, several accounts, spell checking
- [Configuration](docs/configuration.md): the config file, your own colours and keys
- [Installing](docs/install.md): building, packages, other systems
- [Updates](docs/updates.md): how mothrix updates itself, and why that can be trusted
- [The iPhone app](docs/ios.md): how it works, building it, push notifications
- [AGENTS.md](AGENTS.md): how the code is laid out, for changing it

## Development

```sh
cargo test --workspace             # tests, including screen rendering and mouse clicks
dev/server.sh                      # a throwaway homeserver with people, rooms and polls
mothrix --snapshot 8 --room NAME   # sync for 8 s, then print the screen as text
```

[AGENTS.md](AGENTS.md) has the rest: the layout of the code, how to try a
change against the test server, and the things that are not obvious.

## Developed with AI

mothrix is developed with the help of AI. Most of its code, tests and
documentation were written by an AI model (Claude, through Claude Code),
with a person deciding what to build, reviewing the changes, and trying
them out in the terminal and on the phone. The code reviews and security
audits in its history were also done with AI, not by an independent
auditor.

That is one more reason for the warning at the top: read the code before
you rely on it, and report what looks wrong. Every change goes through
the same tests (`cargo test --workspace`, with screens rendered and
clicked), lints and CI as any other.

## Thanks

Built on
[matrix-rust-sdk](https://github.com/matrix-org/matrix-rust-sdk) and
[ratatui](https://ratatui.rs).

## License

MPL-2.0, see [LICENSE](LICENSE).
