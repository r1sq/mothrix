// SPDX-License-Identifier: MPL-2.0

//! The help dialog (`F1`, `?`, `/help`): every key and command, in
//! sections, narrowed by what is typed. The shortcuts that can be rebound
//! and the slash commands come from their own tables ([`crate::keys::ACTIONS`]
//! with the user's bindings, [`COMMANDS`]); the rest is written here, and
//! the tests check that every message action and command is in it.

use super::*;

/// The help dialog: what is typed to narrow it, and how far it is
/// scrolled.
#[derive(Clone, Debug, Default)]
pub struct Help {
    pub filter: Input,
    pub scroll: usize,
}

/// One line of the help.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelpLine {
    Section(&'static str),
    Entry { keys: String, what: String },
}

const ROOM_LIST: &[(&str, &str)] = &[
    ("↑ ↓  j k", "move"),
    ("Enter", "open the room"),
    ("/", "find a room by name"),
    ("p", "pin to the top, or unpin"),
    ("l", "low priority, or normal"),
    ("i", "the room's info"),
    ("M", "change its notifications"),
    ("s", "sort: recent, name, unread first"),
    ("a", "go through the accounts"),
    ("m", "rooms and people in one list, or two"),
    ("g", "put the room into one of your sections"),
    ("G", "rename, reorder or remove your sections"),
    ("z", "fold the section the cursor is in"),
    ("Z", "unfold all"),
    ("right-click", "the room's menu"),
];

const CONVERSATION: &[(&str, &str)] = &[
    ("↑", "from the message box: select the newest message"),
    ("↑ ↓  j k", "select another message"),
    ("End  G", "back to the newest, nothing selected"),
    ("u", "the first new message"),
    ("Enter", "reply to the selected message"),
    ("i", "back to the message box"),
    ("Esc", "a step back"),
];

/// What can be done to the selected message (`message_actions`; each
/// only where it works).
const MESSAGE: &[(&str, &str)] = &[
    ("r", "reply"),
    ("a", "react: then 1-9 your favourites, 0 all emoji"),
    ("e", "edit (your own)"),
    ("d", "delete (asks first)"),
    ("o", "open the attachment or the link"),
    ("c", "copy the text"),
    ("y", "copy a code block"),
    (
        "x",
        "show a long message in full, or fold it; who reacted or read it",
    ),
    ("p", "pin for everyone, or unpin"),
    ("f", "forward"),
    ("t", "open its thread, or start one"),
];

const MESSAGE_BOX: &[(&str, &str)] = &[
    ("Enter", "send"),
    ("Alt+Enter", "a new line"),
    (
        "@name",
        "suggests members: ↑ ↓ choose, Tab or Enter completes",
    ),
    (":name", "suggests emoji the same way"),
    ("/", "suggests the commands below"),
    (
        "Esc",
        "stop replying or editing; else leave the thread, or go to the room list",
    ),
    (
        "F6 or + format",
        "format the word or line at the cursor (b bold, i italic, k code block …)",
    ),
];

const EMOJI_PICKER: &[(&str, &str)] = &[
    ("type", "search by name (heart, party, tada)"),
    ("arrows  PgUp PgDn  Home End", "move"),
    ("Enter or a click", "take it"),
    ("Esc", "close"),
];

const MOUSE: &[(&str, &str)] = &[
    ("click", "open a room, select a message, press a button"),
    ("double-click", "open a file or picture"),
    ("click a header", "fold or unfold a section"),
    ("right-click", "a room's menu"),
    ("wheel", "scroll the room list or the conversation"),
    ("click a link", "open it"),
];

/// What each shortcut that can be rebound does ([`crate::keys::ACTIONS`]).
fn shortcut(name: &str) -> &'static str {
    match name {
        "quit" => "quit",
        "find_room" => "find a room by name",
        "search" => "search messages",
        "settings" => "settings",
        "accounts" => "with several accounts: all of them, or one at a time",
        "unread" => "only unread chats, or all",
        "emoji" => "the emoji picker, for the message box",
        "format" => "the format menu: bold, code, headings, lists … at the cursor",
        "theme" => "the next theme",
        "paste" => "paste a picture or text from the clipboard",
        "previous_room" => "the room above",
        "next_room" => "the room below",
        "switch_pane" => "between the room list and the message box",
        "scroll_up" => "scroll the conversation up (PgDn: down)",
        _ => "",
    }
}

/// Whether every word of `needle` (lower case) begins a word of `line`:
/// "pin" finds "pins" and "pinned", not "typing"; keys like "/" or "?"
/// match where they stand.
fn matches(line: &str, needle: &str) -> bool {
    let line = line.to_lowercase();
    let words: Vec<&str> = line
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',' || c == ':')
        .filter(|word| !word.is_empty())
        .collect();
    needle.split_whitespace().all(|part| {
        words.iter().any(|word| {
            word.starts_with(part) || word.trim_start_matches(['/', '@', ':']).starts_with(part)
        })
    })
}

/// A key as the help shows it: "Ctrl+K", "Alt+↑", "F2".
pub fn key_label(key: &KeyEvent) -> String {
    let mut label = String::new();
    for (modifier, name) in [
        (KeyModifiers::CONTROL, "Ctrl+"),
        (KeyModifiers::ALT, "Alt+"),
        (KeyModifiers::SHIFT, "Shift+"),
    ] {
        if key.modifiers.contains(modifier) {
            label.push_str(name);
        }
    }
    label.push_str(&match key.code {
        KeyCode::Char(' ') => "Space".to_owned(),
        KeyCode::Char(c) => c.to_uppercase().to_string(),
        KeyCode::F(n) => format!("F{n}"),
        KeyCode::Up => "↑".to_owned(),
        KeyCode::Down => "↓".to_owned(),
        KeyCode::Left => "←".to_owned(),
        KeyCode::Right => "→".to_owned(),
        KeyCode::PageUp => "PgUp".to_owned(),
        KeyCode::PageDown => "PgDn".to_owned(),
        other => format!("{other:?}"),
    });
    label
}

impl App {
    /// Open the help, or close it if it is open.
    pub(super) fn toggle_help(&mut self) {
        if self.overlays.is_open::<Help>() {
            self.overlays.close::<Help>();
        } else {
            self.overlays.open(Help::default());
        }
    }

    /// All of the help, in sections.
    pub fn help_lines(&self) -> Vec<HelpLine> {
        let entry = |keys: &str, what: &str| HelpLine::Entry {
            keys: keys.to_owned(),
            what: what.to_owned(),
        };
        let mut lines = vec![HelpLine::Section("Everywhere")];
        lines.push(entry("F1  ?  /help", "this help (? not while typing)"));
        for (name, built_in) in crate::keys::ACTIONS {
            let Some(built_in) = crate::keys::parse_key(built_in) else {
                continue;
            };
            // With the keys the config adds for it.
            let mut keys = key_label(&built_in);
            for (own, stands_for) in &self.keymap {
                if *stands_for == built_in {
                    keys.push_str(&format!(" or {}", key_label(own)));
                }
            }
            lines.push(entry(&keys, shortcut(name)));
        }
        lines.push(entry("Esc", "close a dialog"));
        for (section, entries) in [
            ("In the room list", ROOM_LIST),
            ("In the conversation", CONVERSATION),
            ("On the selected message", MESSAGE),
            ("In the message box", MESSAGE_BOX),
            ("In the emoji picker", EMOJI_PICKER),
            ("With the mouse", MOUSE),
        ] {
            lines.push(HelpLine::Section(section));
            lines.extend(entries.iter().map(|(keys, what)| entry(keys, what)));
        }
        lines.push(HelpLine::Section("Commands in the message box"));
        for (name, what, _) in COMMANDS {
            // "<action>  say what you are doing": the argument goes with
            // the command.
            let (argument, what) = what.split_once("  ").unwrap_or(("", what));
            let command = format!("/{name} {argument}");
            lines.push(entry(command.trim_end(), what));
        }
        lines
    }

    /// The help as it is shown: the lines that match what is typed, under
    /// the headings of their sections.
    pub fn help_shown(&self) -> Vec<HelpLine> {
        let needle = self
            .overlays
            .get::<Help>()
            .map(|help| help.filter.text().trim().to_lowercase())
            .unwrap_or_default();
        if needle.is_empty() {
            return self.help_lines();
        }
        let mut shown = Vec::new();
        let mut section = None;
        for line in self.help_lines() {
            match line {
                HelpLine::Section(_) => section = Some(line),
                HelpLine::Entry { ref keys, ref what } => {
                    if matches(&format!("{keys} {what}"), &needle) {
                        shown.extend(section.take());
                        shown.push(line);
                    }
                }
            }
        }
        shown
    }

    pub(super) fn on_help_key(&mut self, key: KeyEvent) {
        let Some(help) = self.overlays.get_mut::<Help>() else {
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::F(1) => self.overlays.close::<Help>(),
            KeyCode::Up => help.scroll = help.scroll.saturating_sub(1),
            KeyCode::Down => help.scroll += 1,
            KeyCode::PageUp => help.scroll = help.scroll.saturating_sub(10),
            KeyCode::PageDown => help.scroll += 10,
            KeyCode::Home => help.scroll = 0,
            KeyCode::End => help.scroll = usize::MAX,
            _ => {
                help.filter.edit(key);
                help.scroll = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(app: &App) -> String {
        app.help_lines()
            .into_iter()
            .map(|line| match line {
                HelpLine::Section(title) => format!("## {title}\n"),
                HelpLine::Entry { keys, what } => format!("{keys} | {what}\n"),
            })
            .collect()
    }

    #[test]
    fn every_shortcut_command_and_message_action_is_in_the_help() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        let help = text(&app);
        for (name, _) in crate::keys::ACTIONS {
            assert!(
                !shortcut(name).is_empty(),
                "no help for the shortcut {name}"
            );
        }
        for (name, _, _) in COMMANDS {
            assert!(
                help.contains(&format!("/{name}")),
                "/{name} is not in the help"
            );
        }
        // A message with everything: own, with a link and code, long.
        let mut item = Item {
            key: "$1".into(),
            event_id: "$1".into(),
            body: "see https://x.example\n```\ncode\n```".into(),
            kind: ItemKind::Text,
            can_edit: true,
            can_delete: true,
            ..Item::default()
        };
        app.state.prefs.long_messages = 1;
        assert!(crate::ui::can_expand(&app, &item));
        item.pinned = false;
        let message: Vec<&str> = MESSAGE.iter().map(|(key, _)| *key).collect();
        for action in app.message_actions(&item) {
            assert!(
                message.contains(&action.key.to_string().as_str()),
                "{} is not in the help",
                action.key
            );
        }
    }

    #[test]
    fn own_bindings_show_next_to_the_built_in_key() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        let config = std::collections::BTreeMap::from([("search".to_owned(), "alt+s".to_owned())]);
        app.keymap = crate::keys::keymap(&config).0;
        assert!(text(&app).contains("Ctrl+F or Alt+S | search messages"));
    }

    #[test]
    fn f1_opens_and_closes_it_over_anything_and_typing_narrows_it() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.overlays.open(SettingsDialog::default());
        app.on_event(Event::Key(KeyEvent::from(KeyCode::F(1))));
        assert!(matches!(app.overlays.top(), Some(Overlay::Help(_))));
        for c in "thread".chars() {
            app.on_event(Event::Key(KeyEvent::from(KeyCode::Char(c))));
        }
        let shown = app.help_shown();
        assert!(shown.len() >= 2, "{shown:?}");
        assert!(shown.iter().all(|line| match line {
            HelpLine::Entry { keys, what } =>
                format!("{keys} {what}").to_lowercase().contains("thread"),
            HelpLine::Section(_) => true,
        }));
        assert!(matches("/pins show the pinned messages", "pin"));
        assert!(matches("p pin to the top", "pin"));
        assert!(!matches("this help (? not while typing)", "pin"));
        assert!(matches("ctrl+f search messages", "search mess"));
        app.on_event(Event::Key(KeyEvent::from(KeyCode::F(1))));
        assert!(!app.overlays.is_open::<Help>());
        assert!(app.overlays.is_open::<SettingsDialog>());
    }

    #[test]
    fn question_mark_opens_it_except_while_typing() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.focus = Focus::Sidebar;
        app.on_event(Event::Key(KeyEvent::from(KeyCode::Char('?'))));
        assert!(app.overlays.is_open::<Help>());
        app.overlays.close::<Help>();
        app.focus = Focus::Composer;
        app.on_event(Event::Key(KeyEvent::from(KeyCode::Char('?'))));
        assert!(!app.overlays.is_open::<Help>());
        assert_eq!(app.composer.text(), "?");
    }
}
