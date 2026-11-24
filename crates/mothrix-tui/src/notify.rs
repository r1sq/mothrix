// SPDX-License-Identifier: MPL-2.0

//! Desktop notifications about new messages, and clicks on them.
//!
//! In kitty the terminal shows the notification itself (its OSC 99
//! protocol). A click brings kitty's window and tab with mothrix to the
//! front, and kitty tells mothrix which notification it was, so that the
//! room opens. kitty's answer arrives as typed input, which crossterm
//! reads as keys: [`ClickReader`] picks it out again.
//!
//! On Windows a toast shows it; a click opens the room and brings the
//! terminal's window to the front. Elsewhere `notify-send` (Linux) or
//! `osascript` (macOS) show it, and a click does nothing special.
//!
//! Both show the moth as the notification's picture: kitty gets it inside
//! the first notification (and keeps it under [`ICON_KEY`]), `notify-send`
//! as a file in mothrix's cache folder.

use std::collections::HashMap;

use base64::Engine;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

/// Where clicks on toasts send the key of their room.
pub type Clicks = tokio::sync::mpsc::UnboundedSender<String>;

/// Shows notifications and remembers which room each kitty one is about.
pub struct Notifier {
    kitty: bool,
    clicks: Clicks,
    /// kitty has the moth already (it keeps it while it runs).
    icon_sent: bool,
    next_id: u32,
    /// Notifications on screen, by id: the room key each is about.
    shown: HashMap<u32, String>,
}

impl Notifier {
    /// For the terminal mothrix runs in. Clicks on toasts go to `clicks`.
    pub fn new(clicks: Clicks) -> Self {
        let var = |name: &str| std::env::var(name).unwrap_or_default();
        let kitty = !var("KITTY_WINDOW_ID").is_empty() || var("TERM").contains("kitty");
        Self::with_kitty(kitty, clicks)
    }

    fn with_kitty(kitty: bool, clicks: Clicks) -> Self {
        Self {
            kitty,
            clicks,
            icon_sent: false,
            next_id: 1,
            shown: HashMap::new(),
        }
    }

    /// Tell about new messages in `room` (a key), called `title`. Returns
    /// escape sequences for the terminal, where it shows the notification.
    pub fn show(&mut self, room: &str, title: &str) -> Option<String> {
        if !self.kitty {
            spawn_desktop_notification(room, title, &self.clicks);
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.shown.insert(id, room.to_owned());
        let mut out = String::new();
        if !self.icon_sent {
            self.icon_sent = true;
            out.push_str(&kitty_icon(id, ICON));
        }
        out.push_str(&kitty_notification(id, title, BODY));
        Some(out)
    }

    /// The room a click on notification `id` is about.
    pub fn clicked(&mut self, id: u32) -> Option<String> {
        self.shown.remove(&id)
    }

    /// The room was opened: take its notifications off the screen.
    pub fn opened(&mut self, room: &str) -> Option<String> {
        let ids: Vec<u32> = self
            .shown
            .iter()
            .filter(|(_, shown)| *shown == room)
            .map(|(id, _)| *id)
            .collect();
        if ids.is_empty() {
            return None;
        }
        let mut out = String::new();
        for id in ids {
            self.shown.remove(&id);
            out.push_str(&format!("\x1b]99;i={id}:p=close;\x1b\\"));
        }
        Some(out)
    }
}

const BODY: &str = "New message";

/// The moth, 128 pixels square.
const ICON: &[u8] = include_bytes!("../../../assets/mothrix-128.png");

/// The name kitty keeps the moth under, for the notifications after the
/// first (its letters are the ones the protocol allows).
const ICON_KEY: &str = "mothrix-moth-128-1";

fn base64(text: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(text)
}

/// kitty's escape sequences for a notification with a title and a body.
/// A click focuses kitty's window and reports the id back (`a=`). The
/// texts are sent in base64 (`e=1`), so nothing in a room name can end the
/// sequence early.
/// Every notification names the moth (`g=`); kitty shows the picture it
/// keeps under that name, if any.
fn kitty_notification(id: u32, title: &str, body: &str) -> String {
    format!(
        "\x1b]99;i={id}:d=0:e=1:a=focus,report:g={ICON_KEY}:f={app};{title}\x1b\\\
         \x1b]99;i={id}:d=1:e=1:p=body;{body}\x1b\\",
        app = base64("mothrix"),
        title = base64(title),
        body = base64(body),
    )
}

/// The picture `png` for notification `id`, to be kept under
/// [`ICON_KEY`]: in pieces of at most 2048 bytes before base64, as the
/// protocol asks, all before the notification is done (`d=0`).
fn kitty_icon(id: u32, png: &[u8]) -> String {
    png.chunks(2048)
        .map(|piece| {
            format!(
                "\x1b]99;i={id}:d=0:e=1:g={ICON_KEY}:p=icon;{}\x1b\\",
                base64::engine::general_purpose::STANDARD.encode(piece)
            )
        })
        .collect()
}

/// The moth as a file for `notify-send`, in mothrix's cache folder.
#[cfg(not(windows))]
fn icon_file() -> Option<std::path::PathBuf> {
    let dir = mothrix_core::folders::cache()?;
    let file = dir.join("mothrix.png");
    if std::fs::read(&file).ok().as_deref() != Some(ICON) {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&file, ICON).ok()?;
    }
    Some(file)
}

/// A toast on Windows: a click opens `room` and brings the terminal to
/// the front.
#[cfg(windows)]
fn spawn_desktop_notification(room: &str, title: &str, clicks: &Clicks) {
    let (room, title, clicks) = (room.to_owned(), title.to_owned(), clicks.clone());
    std::thread::spawn(move || {
        mothrix_core::toast::show(&title, move || {
            bring_terminal_to_front();
            let _ = clicks.send(room.clone());
        });
    });
}

/// The window of the terminal mothrix runs in, to the front. Windows
/// Terminal's own window owns the one the console reports (since 1.17);
/// the old console host's is that one itself. Best effort: Windows may
/// only make it flash in the taskbar.
#[cfg(windows)]
fn bring_terminal_to_front() {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GA_ROOTOWNER, GetAncestor, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    // SAFETY: plain calls on window handles; a null one is checked first,
    // and the others accept any handle, failing for ones that are gone.
    unsafe {
        let console = GetConsoleWindow();
        if console.is_null() {
            return;
        }
        let owner = GetAncestor(console, GA_ROOTOWNER);
        let window = if owner.is_null() { console } else { owner };
        if IsIconic(window) != 0 {
            ShowWindow(window, SW_RESTORE);
        }
        SetForegroundWindow(window);
    }
}

/// `notify-send` or `osascript`. Best effort: without them, or over SSH,
/// this does nothing; a click does nothing special.
#[cfg(not(windows))]
fn spawn_desktop_notification(_room: &str, title: &str, _clicks: &Clicks) {
    let mut command = if cfg!(target_os = "macos") {
        // The room name travels as an argument, not as script text.
        let mut command = std::process::Command::new("osascript");
        command.args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 1 of argv) with title \"mothrix\"",
            "-e",
            "end run",
            title,
        ]);
        command
    } else {
        let mut command = std::process::Command::new("notify-send");
        let icon = icon_file().map_or_else(
            || "mail-unread".to_owned(),
            |file| file.display().to_string(),
        );
        // "--": a room's name is someone else's text and may start with
        // a dash, which would make it an option.
        command.args([
            "--app-name=mothrix",
            &format!("--icon={icon}"),
            "--",
            title,
            BODY,
        ]);
        command
    };
    if let Ok(mut child) = command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        // Waited for, so that it does not stay behind as a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Terminal input, with kitty's answers to notification clicks taken out.
#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    Event(Event),
    /// Notification `id` was clicked.
    Clicked(u32),
}

/// Finds kitty's answer to a click, `ESC ] 99 ; i=<id> ; ESC \`, among the
/// events crossterm makes of it: Alt+`]`, the characters, then Alt+`\`
/// (or Ctrl+G, for a BEL at the end). Anything else passes unchanged; a
/// start that does not go on like the answer is handed on as typed.
#[derive(Default)]
pub struct ClickReader {
    held: Vec<Event>,
    text: String,
}

/// Longer than any answer kitty sends: give up and pass the keys on.
const LONGEST_ANSWER: usize = 64;

impl ClickReader {
    pub fn feed(&mut self, event: Event) -> Vec<Input> {
        let key = match &event {
            Event::Key(key) => Some(*key),
            _ => None,
        };
        if self.held.is_empty() {
            return match key {
                Some(KeyEvent {
                    code: KeyCode::Char(']'),
                    modifiers,
                    ..
                }) if modifiers.contains(KeyModifiers::ALT) => {
                    self.held.push(event);
                    Vec::new()
                }
                _ => vec![Input::Event(event)],
            };
        }
        let end = matches!(
            key,
            Some(KeyEvent { code: KeyCode::Char('\\'), modifiers, .. })
                if modifiers.contains(KeyModifiers::ALT)
        ) || matches!(
            key,
            Some(KeyEvent { code: KeyCode::Char('g'), modifiers, .. })
                if modifiers.contains(KeyModifiers::CONTROL)
        );
        if end {
            let answer = std::mem::take(&mut self.text);
            let held = std::mem::take(&mut self.held);
            return match click_id(&answer) {
                Some(id) => vec![Input::Clicked(id)],
                // Some other answer of the terminal: nothing for us.
                None if answer.starts_with("99;") => Vec::new(),
                None => held.into_iter().chain([event]).map(Input::Event).collect(),
            };
        }
        let typed = match key {
            Some(KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            }) if !modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) => Some(c),
            _ => None,
        };
        if let Some(c) = typed {
            self.text.push(c);
            self.held.push(event);
            let answer_so_far =
                "99;".starts_with(self.text.as_str()) || self.text.starts_with("99;");
            if answer_so_far && self.text.len() <= LONGEST_ANSWER {
                return Vec::new();
            }
        } else {
            self.held.push(event);
        }
        // Not an answer after all.
        self.text.clear();
        std::mem::take(&mut self.held)
            .into_iter()
            .map(Input::Event)
            .collect()
    }
}

/// The id in an answer like `99;i=7;` (metadata may hold more keys).
fn click_id(answer: &str) -> Option<u32> {
    let metadata = answer.strip_prefix("99;")?.split(';').next()?;
    metadata
        .split(':')
        .find_map(|pair| pair.strip_prefix("i="))?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn chars(text: &str) -> Vec<Event> {
        text.chars()
            .map(|c| key(KeyCode::Char(c), KeyModifiers::NONE))
            .collect()
    }

    /// What crossterm makes of `ESC ] <text> ESC \`.
    fn answer(text: &str) -> Vec<Event> {
        let mut events = vec![key(KeyCode::Char(']'), KeyModifiers::ALT)];
        events.extend(chars(text));
        events.push(key(KeyCode::Char('\\'), KeyModifiers::ALT));
        events
    }

    fn feed_all(reader: &mut ClickReader, events: Vec<Event>) -> Vec<Input> {
        events.into_iter().flat_map(|e| reader.feed(e)).collect()
    }

    #[test]
    fn a_click_answer_becomes_a_click_and_nothing_is_typed() {
        let mut reader = ClickReader::default();
        let mut events = chars("hi");
        events.extend(answer("99;i=12;"));
        events.extend(chars("!"));
        let inputs = feed_all(&mut reader, events);
        assert_eq!(
            inputs,
            [
                Input::Event(key(KeyCode::Char('h'), KeyModifiers::NONE)),
                Input::Event(key(KeyCode::Char('i'), KeyModifiers::NONE)),
                Input::Clicked(12),
                Input::Event(key(KeyCode::Char('!'), KeyModifiers::NONE)),
            ]
        );
    }

    #[test]
    fn a_bel_ends_the_answer_too_and_more_metadata_is_fine() {
        let mut reader = ClickReader::default();
        let mut events = vec![key(KeyCode::Char(']'), KeyModifiers::ALT)];
        events.extend(chars("99;i=3:p=title;"));
        events.push(key(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert_eq!(feed_all(&mut reader, events), [Input::Clicked(3)]);
    }

    #[test]
    fn alt_bracket_typed_by_hand_still_arrives() {
        let mut reader = ClickReader::default();
        let mut events = vec![key(KeyCode::Char(']'), KeyModifiers::ALT)];
        events.extend(chars("x"));
        let inputs = feed_all(&mut reader, events.clone());
        assert_eq!(
            inputs,
            events.into_iter().map(Input::Event).collect::<Vec<_>>()
        );
        // And then a plain key goes straight through.
        assert_eq!(
            reader.feed(key(KeyCode::Enter, KeyModifiers::NONE)),
            [Input::Event(key(KeyCode::Enter, KeyModifiers::NONE))]
        );
    }

    #[test]
    fn notifications_carry_the_room_and_close_when_it_is_opened() {
        let mut notifier = Notifier::with_kitty(true, tokio::sync::mpsc::unbounded_channel().0);
        let sequence = notifier.show("work/!a", "Stand;up\x1b").unwrap();
        // The title travels in base64: no stray terminator or separator.
        assert!(sequence.contains(&base64("Stand;up\x1b")));
        assert!(sequence.contains("\x1b]99;i=1:d=0:e=1:a=focus,report:"));
        let pieces = ICON.len().div_ceil(2048);
        assert_eq!(sequence.matches("\x1b\\").count(), pieces + 2);

        // The moth went with the first; the next only names it.
        let second = notifier.show("!b", "Lunch").unwrap();
        assert!(!second.contains("p=icon"));
        assert!(second.contains(&format!("g={ICON_KEY}:")));
        assert_eq!(notifier.clicked(2).as_deref(), Some("!b"));
        assert_eq!(notifier.clicked(2), None);
        assert_eq!(
            notifier.opened("work/!a").as_deref(),
            Some("\x1b]99;i=1:p=close;\x1b\\")
        );
        assert_eq!(notifier.opened("work/!a"), None);
    }

    #[test]
    fn the_moth_goes_to_kitty_in_pieces_that_add_up() {
        let sequence = kitty_icon(4, ICON);
        let mut png = Vec::new();
        for piece in sequence.split_terminator("\x1b\\") {
            let (meta, data) = piece.split_once(';').unwrap().1.split_once(';').unwrap();
            assert_eq!(meta, format!("i=4:d=0:e=1:g={ICON_KEY}:p=icon"));
            assert!(data.len() <= 4096);
            png.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap(),
            );
        }
        assert_eq!(png, ICON);
        assert!(png.starts_with(b"\x89PNG"));
    }
}
