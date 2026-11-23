# Using mothrix

How to get around: keys, commands, several accounts, spell checking. What
there is to find is in [features.md](features.md); the config file in
[configuration.md](configuration.md).

## Keys

| Key | Action |
|-----|--------|
| `F1`, `?` or `/help` | every key and command, narrowed by what you type (`?` not while typing) |
| `Tab` | switch between the room list and the message box |
| `Ctrl+K` or `/` | find a room |
| `Ctrl+F` | search messages |
| `Ctrl+V` | paste a picture or text from the clipboard |
| `F2` | settings |
| `F3` | with several accounts: all of them, or one at a time |
| `F4` | only unread chats, or all |
| `F5` | emoji picker: search and pick an emoji for the message box |
| `F6` | format menu: bold, italic, strikethrough, code, code block, heading, quote, lists, link, for the word or line at the cursor |
| `Alt+↑` `Alt+↓` | previous / next room |
| `PgUp` `PgDn` | scroll the conversation |
| `Ctrl+T` | next theme |
| `Ctrl+Q` | quit |

**In the room list:** `↑` `↓` / `j` `k` move, `Enter` opens, `p` pins or
unpins, `l` toggles low priority, `i` shows the room's info, `M` changes
its notifications, `s` changes the sort order, `a` goes through the
accounts, `m` mixes rooms and people into one list, `g` chooses its
section, `G` manages the sections, `z` folds the current section, `Z`
unfolds all.

**In the conversation:** `↑` from the message box selects the newest
message, `↑` `↓` move the selection, then `r` reply, `a` react (then
`1`-`9` for your favourites, `0` for all emoji), `e` edit, `c` copy, `p`
pin, `f` forward, `u` first new message, `t` open its thread (or start
one), `y` copy code, `x` unfold or fold a long message and the names of
who reacted or read it, `d` delete, `o` open attachment, `Esc` back.

**While typing:** `Alt+Enter` starts a new line. `@name` opens the member
suggestions (`↑` `↓` choose, `Tab` or `Enter` completes, `Esc` closes);
`:name` does the same for emoji. Clicking into the text places the cursor.
`F6` or the `+ format` button opens the format menu: a letter (`b` bold,
`i` italic, `s` strikethrough, `c` code, `k` code block, `h` heading, `q`
quote, `l` list, `n` numbered list, `u` link) formats the word at the
cursor, or the start of its line; doing it again takes it away. Messages
are sent as Markdown, so typing the marks yourself works the same.

**With the mouse**, a double-click on a file or picture opens it (as `o`).

**In the emoji picker** (`F5`, the `+ emoji` button, or `0` on the
reaction row): type to search by name (`heart`, `party`, `tada`), arrows
move, `PgUp` `PgDn` `Home` `End` jump, `Enter` or a click picks, `Esc`
closes.

Every shortcut in the table can get a second key of your choice: see
[configuration.md](configuration.md#your-own-keys).

## Commands

Typing `/` in the message box lists the commands; `Tab` completes one,
`Enter` completes it and runs it if it needs nothing more.

| Command | |
|---|---|
| `/me <action>` | an emote |
| `/poll Question \| Answer 1 \| Answer 2 [--multi] [--hidden]` | a poll |
| `/file <path> [\| caption]`, `/paste` | send a file, the clipboard's picture |
| `/search [words]` | search messages |
| `/info`, `/pins` | the room's info, its pinned messages |
| `/invite <who>`, `/dm <who>`, `/room <name>`, `/join <#room:server>`, `/leave` | rooms and people |
| `/verify [who]`, `/recover <key>`, `/backup [new]`, `/reset-identity` | encryption |
| `/update` | install a new release of mothrix ([updates.md](updates.md)) |
| `/settings`, `/accounts`, `/theme <name>`, `/stop`, `/quit` | the rest |

`<who>` is a Matrix ID, or a name completed with `@`.

## Several accounts

Inside mothrix: `F2`, then **Accounts** (or `/accounts`). The dialog lists
who is signed in, with connection and verification state; `a` adds an
account, `r` gives one a name of its own for the tabs, `K` and `J` move
one up or down, `d` signs one out (and deletes its data on this
computer). Adding asks for the server, then user and password; leave
those two empty to sign in through the browser where the server offers
it.

From the command line:

```sh
mothrix login school.example.org --profile school   # add an account
mothrix accounts                                    # list them
mothrix                                             # start with all of them
mothrix --profile school                            # start with only this one
mothrix logout --profile school                     # remove it again
```

The first account is the profile `default`. With more than one, the room
list gets a row of tabs: `all` shows every account under a line with its
name (click the line to fold it), a name shows only that account; `a` in
the room list or `F3` goes through them. What you write goes out from the
account the open room belongs to, and the status bar shows who that is.

An account is used by one mothrix at a time: a second one started with
the same account says so and leaves it out, since both would write the
same encryption keys.

## Spell checking

mothrix uses the Hunspell dictionaries installed on the system
(`/usr/share/hunspell`, `/usr/share/myspell`), by default the system
language plus US English. If one is missing:

```sh
mothrix dictionary            # what is installed, in use, and downloadable
mothrix dictionary de_DE      # download into ~/.local/share/mothrix/dictionaries
```

Choose the languages with `spell_languages = ["de_DE", "en_US"]` in the
config file. Misspelled words are underlined; put the cursor on one (or
click it) for corrections, `Tab` applies the first. Ignored words are kept
in `~/.config/mothrix/words.txt`.

### LanguageTool

For grammar and style on top of spelling, name a
[LanguageTool](https://languagetool.org) server: in the settings (`F2`,
"LanguageTool server") or as `languagetool_url = "https://lt.example.org"`
in the config file. While you pause typing, the message box's text is sent
to that server, so use one you trust (self-hosting is one
`docker run erikvl87/languagetool`). Its findings are underlined in a
second colour. If the server cannot be reached, the built-in check
carries on.

By default the server detects the language of each message. To pin it
down, set "LanguageTool languages" (or `languagetool_languages = ["de",
"en"]`); with several, every sentence keeps the language that fits it.

For the paid service, set the server to `https://api.languagetoolplus.com`
and enter your account e-mail and API key ("LanguageTool user" and
"LanguageTool API key" in the settings). The key is kept in the config
file, which is readable only by you.
