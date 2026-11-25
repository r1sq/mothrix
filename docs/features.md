# What mothrix does

Everything the terminal client can do, by topic. Keys and commands are in
[usage.md](usage.md); the iPhone app is in [ios.md](ios.md).

## The room list

- Invitations, pinned chats, rooms, direct chats and low-priority rooms,
  with unread and mention counts
- An invitation is joined by opening it twice in a row: the first time
  mothrix says what it is, and any other key or click (or a minute)
  takes the question back; its menu's "Leave…" turns it down. Only your
  own key, click or tap joins: a room that opens by itself (the one open
  last time, which is an invitation now) is marked in the list instead
- Right-click a room or person for its menu: open, pin, move to a
  section, low priority, notifications
- Pin a room or person to the top (the menu, or `p`); pins use the
  standard Matrix "favourite" tag, so other clients show them too
- Rooms and people in separate sections, or mixed in one list (`m`, or in
  the settings); fold a section by clicking its header or with `z` (`Z`
  unfolds all); a folded section shows its room and unread counts
- Your own sections in the mixed list: `g` puts a room or person into a
  section you name ("Work", "Family", …); `G` (or a right-click on a
  section header) renames, reorders or removes them. Subsections one level
  deep: `Work/Clients` sits indented under `Work`
- Sections travel with the account (its account data on the homeserver,
  not encrypted): every device of it, the iPhone app too, lists rooms
  alike, under the same rules: names match regardless of case, a section
  goes once nothing is left in it, and a folded section stays folded
  when it is renamed. A change made without network is sent when it is
  back, unless another device changed the sections meanwhile: then
  theirs stand
- Sort each section by recent activity, name, or unread first (`s`, or
  click `sort:` below the list)
- Spaces: their rooms are listed under the space's name, subspaces one
  level below
- Unread view: `U`, `F4` or the `unread` button shows only chats with
  something new, from all accounts and sections in one list, mentions
  first
- Notifications per room: all messages, mentions only, or muted (`M`);
  muted rooms are dimmed

## The conversation

- Messages grouped by sender, with replies, reactions, edits, room events
  and date lines; links underlined and opened with a click
- Reply, react (your nine favourites, or any emoji from the picker), edit,
  delete (asks first), forward (`f`; into a room of any account; an
  encrypted file not into an unencrypted room), pin for everyone (`p`;
  `/pins` reads them)
- Who reacted and who has read up to where, under a message; who is
  writing right now (both can be switched off, as can telling others that
  you write)
- Threads: replies in a thread stay out of the room; the first message
  shows how many there are, `t` or a click opens the thread
- Markdown both ways: `**bold**`, `` `code` ``, lists, links, quotes,
  headings; code blocks framed, each with a `[copy]` button
- Long messages folded after 15 lines with a `show more` link (the limit
  is a setting)
- `u` jumps to the first new message; what you typed but did not send
  stays with its room, also over a restart

## Pictures, files and links

- Pictures right in the conversation: sharp in kitty and Ghostty (kitty
  graphics) and in terminals that draw Sixel (foot, WezTerm, Konsole,
  Windows Terminal, xterm as VT340), as coloured half blocks in any other
  true-colour terminal
- `o` opens the original, or any file, in the desktop's default app
- Send files with `/file <path>` (`Tab` completes the path), the `+ file`
  button, or by dropping a file onto the terminal, with a caption if you
  like (`/file picture.png | text`); pictures go with their size and a
  small preview
- `Ctrl+V` takes a picture (or text) from the clipboard, `/paste` sends a
  picture right away
- Voice messages and audio play in the background if `mpv` is installed
- Link previews (title and summary), off by default: your homeserver
  fetches the pages and so learns the links; never in encrypted rooms
- A link with a name shows the site it leads to after it: `the menu
  (food.example)`
- Opening an attachment hands pictures, sound, video and documents to the
  desktop's program for them; anything else (programs, scripts, web
  pages, and what is called like sound but does not begin like it) is
  only saved, and mothrix says where

## Polls

- Create them with `/poll` or the `+ poll` button: single or multiple
  choice, results open or hidden until the end; end your own
- Bars, counts and who voted for what; click an answer to vote
- All three formats in use: the older `org.matrix.msc3381.poll.*`
  (Element), the standard `m.poll.*`, and the mix the IServ messenger
  sends; votes go out in the poll's own format, and answers count
  whatever format they arrive in

## Writing

- Several lines: `Alt+Enter` starts a new one, pasted text keeps its line
  breaks
- `@` and a few letters suggests room members; `Tab` makes it a real
  mention that notifies them
- `:name` suggests emoji (`:tada` → 🎉), the ones you take most first; an
  emoji picker (`F5`) with every emoji by group and a search line
- A format menu (`F6`, `+ format`): bold, italic, strikethrough, code,
  code blocks, headings, quotes, lists and links for the word or line at
  the cursor
- A double-click on a file or picture opens it
- Spell checking while you type, several languages at once, offline
  (Hunspell); optional grammar and style checking by a LanguageTool server
  you name (see [usage.md](usage.md#spell-checking))

## Search

- `Ctrl+F` or the `search` button: in the open chat or in all chats;
  `Enter` on a result jumps to the message. It searches what this device
  has loaded (encrypted rooms included) plus the server's search for
  unencrypted history

## Rooms and people

- Room info (`i`, `/info` or the right-click menu): topic, address,
  encryption, and everyone in the room with their role; from there invite,
  start a direct chat, rename the room, set its topic, leave; where you
  moderate, remove or ban people and delete their messages
- `/room <name>` makes a new private (encrypted) room, `/dm <who>` starts
  a direct chat, `/join <#room:server>` joins one, `/invite <who>` invites

## Encryption and verification

- End-to-end encryption, with `/recover <recovery key>` to read older
  messages
- Verify this device against another of yours, or another person, by
  comparing emoji (`/verify`, `v` on them in the room info, or accept the
  request that pops up); "They match" is offered only while all seven
  emoji are on the screen
- A dialog the terminal is too small to show says so and takes no key
  but `Esc`: nothing is confirmed unseen
- Key backup from inside mothrix: `/backup` sets it up and shows the
  recovery key once; `/reset-identity` starts over when no device and no
  key is left
- An account's keys and what it has read are encrypted on disk, with a
  passphrase kept in the system's keyring (GNOME Keyring or KWallet, the
  macOS Keychain, the Windows Credential Manager; on the iPhone its
  Keychain): see [configuration.md](configuration.md)

## Accounts

- Several accounts at once, on the same or different servers: each under
  its own line in the room list, or one at a time through the tabs above
  it (see [usage.md](usage.md#several-accounts))
- Sign-in through the browser (single sign-on) or with a password

## Notifications

- Desktop notifications for new messages, with the moth, via
  `notify-send`; in kitty the terminal shows them itself, and clicking one
  brings mothrix to the front with that chat open. On Windows they are
  toasts; a click opens the chat

## Look and feel

- Mouse for everything: click rooms, messages, poll answers, reactions,
  buttons; the wheel scrolls either pane
- The keyboard for everything as well
- Seven colour themes, by default `mothrix` (the logo's colours), and
  your own (see [configuration.md](configuration.md)); the terminal's own
  background is left alone
- A help (`F1`, `?` or `/help`) with every key and command, narrowed by
  what you type, showing your own key bindings too
- A settings dialog (`F2`) for theme, room order, pictures,
  notifications, spell checking, and the rest; it says which mothrix
  this is (the version, and the commit for a build that is not a
  release), as `mothrix --version` does
- Updates itself: the release files from GitHub look for a new release
  daily and install it on `/update` (or `mothrix update`), checked against
  a signature; packages leave it to the package manager
  ([updates.md](updates.md))

## Not there yet

- Sharp pictures inside terminal multiplexers (tmux, screen: use
  `images = "blocks"` there)
- Room settings beyond name and topic (who may do what, history, access)
- Servers that only offer the newer OAuth login
- The server must support sliding sync (Synapse and Continuwuity do)
