// SPDX-License-Identifier: MPL-2.0

//! UI state and input handling. Pure logic: it turns key, mouse and backend
//! events into state changes plus [`Command`]s for the backend, and never
//! touches the terminal or the network itself.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use crate::{
    client_state::{Opening, Taken},
    input::Input,
    matrix::{Command, SyncStatus, Update},
    model::{Item, ItemKind, Member, Poll, PollFormat, Preview, RoomEntry, VerifyState},
    spell::Speller,
    theme::{self, Theme},
    verify::VerifyInput,
};

mod accounts;
mod actions;
mod dialogs;
mod effects;
mod emoji_picker;
mod format_menu;
mod help;
mod overlay;
mod roominfo;
mod settings;
mod sidebar;
mod suggestions;
mod updates;
mod viewport;

pub use crate::accounts::Account;
pub use effects::Effect;

pub use emoji_picker::{CELL_WIDTH, EmojiPicker, GridRow, PickFor, reveal as emoji_picker_reveal};

pub use overlay::{Overlay, Overlays, RecoveryKey, ResetPrompt, SettingsDialog};

pub use sidebar::{Badge, SidebarRow};

pub use roominfo::{InfoAction, InfoConfirm, InfoInput, InfoMode, RoomInfo};

pub use accounts::{ADD_FIELDS, AccountRequest, AccountsAction, AccountsDialog, AccountsMode};

pub use dialogs::{
    RoomMenu, RoomMenuItem, Search, SectionEdit, SectionManager, SectionPicker, SectionRow,
};

pub use format_menu::FormatMenu;
pub use help::{Help, HelpLine};
pub use settings::Setting;
pub use updates::Updates;

pub use suggestions::Popup;

pub use viewport::Viewport;

/// How many suggestions the @-mention popup shows.
pub const MENTION_ROWS: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Timeline,
    Composer,
}

/// How pictures are drawn in the conversation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Graphics {
    /// Sharp pictures through the kitty graphics protocol.
    Kitty,
    /// Sharp pictures as Sixel graphics.
    Sixel,
    /// Coloured half-block characters; works in any true-colour terminal.
    #[default]
    Blocks,
    Off,
}

impl Graphics {
    /// `auto` (or anything unknown) picks kitty graphics where supported.
    pub fn from_label(label: &str, kitty_supported: bool, sixel_supported: bool) -> Self {
        match label.trim() {
            "kitty" => Self::Kitty,
            "sixel" => Self::Sixel,
            "blocks" => Self::Blocks,
            "off" => Self::Off,
            _ if kitty_supported => Self::Kitty,
            _ if sixel_supported => Self::Sixel,
            _ => Self::Blocks,
        }
    }
}

/// The commands of the message box: (name, what it does, takes an argument).
/// Two clicks on a message within this are a double-click.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

pub const COMMANDS: [(&str, &str, bool); 23] = [
    ("me", "<action>  say what you are doing", true),
    (
        "poll",
        "Question | Answer 1 | Answer 2 [--multi] [--hidden]",
        true,
    ),
    ("file", "<path> [| caption]  send a file", true),
    ("paste", "send the picture on the clipboard", false),
    ("search", "[words]  search messages", true),
    ("settings", "open the settings", false),
    ("help", "every key and command", false),
    ("accounts", "add, rename or sign out accounts", false),
    ("verify", "verify this device with another of yours", false),
    (
        "recover",
        "<recovery key>  read older encrypted messages",
        true,
    ),
    (
        "backup",
        "[new]  set up key backup, or make a new recovery key",
        false,
    ),
    (
        "reset-identity",
        "start over with encryption when all else is lost",
        false,
    ),
    ("theme", "<name>  switch the colours", true),
    ("pins", "show the pinned messages of this room", false),
    ("stop", "stop the audio that is playing", false),
    ("update", "install a new release of mothrix", false),
    (
        "info",
        "who is in this room, and what can be done here",
        false,
    ),
    (
        "invite",
        "<@user:server>  invite someone into this room",
        true,
    ),
    ("dm", "<@user:server>  start a direct chat", true),
    ("room", "<name>  make a new private room", true),
    ("join", "<#room:server>  join a room", true),
    ("leave", "leave this room", false),
    ("quit", "leave mothrix", false),
];

/// A Matrix id cut in the middle to at most `max` characters; some
/// servers hand out very long ones.
pub fn short_id(id: &str, max: usize) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= max.max(5) {
        return id.to_owned();
    }
    let keep = max.max(5) - 1;
    let head: String = chars[..keep.div_ceil(2)].iter().collect();
    let tail: String = chars[chars.len() - keep / 2..].iter().collect();
    format!("{head}…{tail}")
}

/// What Enter in the message box does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Compose {
    Message,
    Reply { event_id: String, label: String },
    Edit { event_id: String },
}

/// The strip shown under the conversation while a message is selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bar {
    Actions,
    Emoji,
    ConfirmDelete,
    ConfirmEndPoll,
}

/// Buttons of the selected-message strip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BarAction {
    /// Copy only the code blocks of the message.
    CopyCode,
    /// Unfold or fold a long message.
    Expand,
    /// Open the thread of the message, or start one.
    Thread,
    /// Pin the message for everyone in the room, or unpin it.
    Pin,
    /// Send a copy into another room.
    Forward,
    Reply,
    React,
    Edit,
    Delete,
    Open,
    /// Put the message text on the clipboard.
    Copy,
    Emoji(String),
    /// Open the emoji picker to react with any emoji.
    MoreEmoji,
    ConfirmDelete,
    ConfirmEndPoll,
    /// Back out of a confirmation or the emoji row; closes the strip otherwise.
    Cancel,
}

/// What a click on a screen region does. Regions are rebuilt on every draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HitAction {
    OpenRoom(String),
    FocusComposer,
    FocusTimeline,
    OpenThread(String),
    CloseThread,
    /// Open this web address in the browser.
    OpenLink(String),
    /// Select (or deselect) the message with this event id.
    Select(String),
    Vote {
        poll_event_id: String,
        answer_id: String,
    },
    React {
        event_id: String,
        key: String,
    },
    AskEndPoll(String),
    Bar(BarAction),
    JumpToBottom,
    NextTheme,
    NextSort,
    /// List only unread chats, or all again.
    ToggleUnread,
    /// Fold or unfold this sidebar section.
    ToggleSection(String),
    /// Show all accounts (`None`) or only one.
    AccountTab(Option<usize>),
    /// Choose this row of the section picker.
    SectionPick(SectionRow),
    SectionEdit(SectionEdit),
    /// Choose this entry of a room's right-click menu.
    RoomMenuPick(RoomMenuItem),
    /// Copy one code block of a message.
    CopyCode {
        event_id: String,
        index: usize,
    },
    /// Show a long message in full, or fold it again.
    ToggleExpand(String),
    /// Put a `/poll` example into the message box.
    PollTemplate,
    /// Start a `/file` line with path suggestions.
    FileTemplate,
    /// Open the emoji picker for the message box.
    OpenEmojiPicker,
    /// Open the format menu (where its button is).
    OpenFormatMenu {
        x: u16,
        y: u16,
    },
    /// Apply this format to the message box.
    FormatPick(crate::format::Format),
    /// Take this emoji of the picker.
    EmojiPick(String),
    /// Complete the @-mention with this suggestion (index into the popup).
    MentionPick(usize),
    Verify(VerifyInput),
    OpenSettings,
    Accounts(AccountsAction),
    RoomInfo(InfoAction),
    /// Forward the chosen message to this room (its key).
    ForwardTo(String),
    /// Change this setting.
    Setting(Setting),
    OpenSearch,
    /// Search this chat (`false`) or all chats (`true`).
    SearchScope(bool),
    /// Open this search result.
    SearchHit {
        room_id: String,
        event_id: String,
    },
    CloseDialogs,
    /// Swallow the click (the area under an open dialog).
    Blocked,
    CancelCompose,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub rect: Rect,
    pub action: HitAction,
}

/// A Sixel picture and the cells it fills on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SixelPlacement {
    pub x: u16,
    pub y: u16,
    pub columns: u16,
    pub rows: u16,
    pub event_id: String,
    /// The message is selected (its background differs).
    pub selected: bool,
}

/// The dialog for choosing where a message is forwarded to.
#[derive(Clone, Debug)]
pub struct Forward {
    pub event_id: String,
    pub filter: Input,
    pub cursor: usize,
}

pub struct App {
    /// What core keeps of the accounts and rooms (not the state file).
    pub state: crate::client_state::ClientState,
    /// New releases of mothrix itself.
    pub updates: updates::Updates,
    /// The message clicked last, and when: for double-clicks.
    last_click: Option<(String, std::time::Instant)>,
    /// What is to be done outside, in order (see [`Effect`]).
    effects: Vec<Effect>,
    /// The dialogs and menus open over the main screen.
    pub overlays: Overlays,
    pub theme: Theme,
    /// Long messages the user unfolded.
    pub expanded: HashSet<String>,
    /// Messages that are long enough to fold, as of the last draw.
    pub collapsible: HashSet<String>,
    /// Which emoji the user takes most, for the reaction bar and `:`.
    pub emoji_usage: crate::emoji::Usage,
    /// Key bindings from the config: (key chosen, built-in key).
    pub keymap: Vec<(KeyEvent, KeyEvent)>,
    /// The text we last told the room we were writing (empty: not writing).
    typing_told: String,
    /// What was typed but not sent, by room.
    pub drafts: HashMap<String, String>,
    /// The room to open again once it is listed (the one open last time).
    reopen: Option<String>,
    /// Drafts or the open room changed; the main loop saves them.
    pub state_dirty: bool,
    /// Room id the sidebar cursor is on (keyboard navigation).
    pub cursor_room: Option<String>,
    /// Cursor position the sidebar last scrolled into view.
    pub sidebar_cursor_seen: Option<String>,
    pub sidebar_scroll: usize,
    pub filter: Input,
    pub filtering: bool,
    /// Which lines of the conversation are in view.
    pub viewport: Viewport,
    /// Event id of the selected message.
    pub selected: Option<String>,
    pub bar: Bar,
    pub composer: Input,
    pub compose: Compose,
    /// Mentions completed in the current composer text.
    composer_mentions: Vec<Member>,
    pub mention_selected: usize,
    /// Esc closed the popup; typing more opens it again.
    mention_hidden: bool,
    /// Downloaded pictures by event id.
    pub previews: HashMap<String, Preview>,
    pub graphics: Graphics,
    /// Pictures uploaded to the terminal (kitty graphics): event id to
    /// the terminal-side picture id.
    pub terminal_images: HashMap<String, u32>,
    next_image_id: u32,
    /// Languages of the loaded dictionaries (empty until loaded).
    pub spell_languages: Vec<String>,
    /// Whether the terminal can show pictures itself (kitty graphics).
    pub kitty_supported: bool,
    /// Whether the terminal is known to draw Sixel graphics.
    pub sixel_supported: bool,
    /// The size of a terminal cell in pixels, for Sixel pictures.
    pub cell_px: (u16, u16),
    /// For which way of drawing pictures (and cell size) the logo was made.
    logo_for: Option<(Graphics, (u16, u16))>,
    /// Pictures as Sixel sequences, by event id.
    pub sixel: HashMap<String, String>,
    /// Where the last draw wants Sixel pictures painted.
    pub sixel_placements: Vec<SixelPlacement>,
    /// A search result waiting for its message to be loaded.
    pending_reveal: Option<String>,
    speller: Option<Arc<dyn Speller>>,
    /// Misspelled words in the message box, as char ranges.
    pub misspelled: Vec<(usize, usize)>,
    /// The marked word under the cursor and its corrections.
    spell_popup: Option<dialogs::SpellPopup>,
    /// What LanguageTool found in the text it last checked.
    grammar: Vec<crate::model::GrammarMatch>,
    grammar_checked: Option<String>,
    grammar_requested: Option<String>,
    grammar_failed: bool,
    /// Grammar and style problems in the message box, as char ranges.
    pub grammar_marks: Vec<(usize, usize)>,
    /// Lowercase words never to mark as misspelled.
    pub ignored_words: HashSet<String>,
    /// (text, cursor, enabled) the current marks were computed for.
    spell_key: Option<(String, usize, bool)>,
    /// Where the message box text was drawn, for click-to-place-cursor.
    pub composer_text_area: Rect,
    pub composer_first_row: usize,
    pub focus: Focus,
    pub notice: Option<String>,
    pub should_quit: bool,

    // Filled in by the last draw; used for mouse hit-testing and scrolling.
    pub hits: Vec<Hit>,
    pub sidebar_area: Rect,
    pub timeline_area: Rect,
}

impl App {
    pub fn new(theme: Theme, user_id: String) -> Self {
        Self {
            state: crate::client_state::ClientState::new(vec![Account {
                profile: "default".to_owned(),
                label: "default".to_owned(),
                user_id,
                own_name: None,
                sync: SyncStatus::Connecting,
                verified: None,
            }]),
            updates: updates::Updates::new(crate::update::Build::Development, false),
            last_click: None,
            overlays: Overlays::default(),
            viewport: Viewport::default(),
            effects: Vec::new(),
            theme,
            expanded: HashSet::new(),
            collapsible: HashSet::new(),
            emoji_usage: crate::emoji::Usage::default(),
            keymap: Vec::new(),
            typing_told: String::new(),
            drafts: HashMap::new(),
            reopen: None,
            state_dirty: false,
            cursor_room: None,
            sidebar_cursor_seen: None,
            sidebar_scroll: 0,
            filter: Input::default(),
            filtering: false,
            selected: None,
            bar: Bar::Actions,
            composer: Input::default(),
            compose: Compose::Message,
            composer_mentions: Vec::new(),
            mention_selected: 0,
            mention_hidden: false,
            previews: HashMap::new(),
            graphics: Graphics::default(),
            terminal_images: HashMap::new(),
            next_image_id: 1,
            spell_languages: Vec::new(),
            kitty_supported: false,
            sixel_supported: false,
            cell_px: (10, 20),
            logo_for: None,
            sixel: HashMap::new(),
            sixel_placements: Vec::new(),
            pending_reveal: None,
            speller: None,
            misspelled: Vec::new(),
            spell_popup: None,
            grammar: Vec::new(),
            grammar_checked: None,
            grammar_requested: None,
            grammar_failed: false,
            grammar_marks: Vec::new(),
            ignored_words: HashSet::new(),
            spell_key: None,
            composer_text_area: Rect::default(),
            composer_first_row: 0,
            focus: Focus::Sidebar,
            notice: None,
            should_quit: false,
            hits: Vec::new(),
            sidebar_area: Rect::default(),
            timeline_area: Rect::default(),
        }
    }

    pub fn open_room_entry(&self) -> Option<&RoomEntry> {
        let id = self.state.open_room.as_deref()?;
        self.state.rooms.iter().find(|room| room.id == id)
    }

    pub fn selected_item(&self) -> Option<&Item> {
        let id = self.selected.as_deref()?;
        self.state.items.iter().find(|item| item.event_id == id)
    }

    /// The account commands without a room go to: the one of the open
    /// room, else the one being shown, else the one the cursor is in.
    pub fn active_account(&self) -> usize {
        crate::accounts::Frontend::active_account(self)
    }

    pub fn account(&self) -> &Account {
        &self.state.accounts[self.active_account()]
    }

    /// Show a long message in full, or fold it again.
    fn toggle_expand(&mut self, event_id: &str) {
        if !self.expanded.remove(event_id) {
            self.expanded.insert(event_id.to_owned());
        }
    }

    fn copy_code(&mut self, event_id: &str, index: Option<usize>) {
        let Some(item) = self
            .state
            .items
            .iter()
            .find(|item| item.event_id == event_id)
        else {
            return;
        };
        let blocks = crate::codeblock::code_blocks(&item.body);
        let text = match index {
            Some(index) => blocks.get(index).cloned(),
            // From the keyboard: all the code in the message.
            None if !blocks.is_empty() => Some(blocks.join("\n\n")),
            None => None,
        };
        if let Some(text) = text {
            self.effects.push(Effect::Copy(text));
            self.notice = Some("Code copied to the clipboard.".to_owned());
        }
    }

    fn open(&mut self, room_id: String, commands: &mut Vec<Command>) {
        let opening = self.state.open(&room_id);
        commands.extend(opening.commands());
        let previous = match opening {
            Opening::NotListed => return,
            Opening::Opened { previous, .. } => previous,
            // Clicking the room again leads out of a thread.
            Opening::Again { left_thread } => {
                self.went_to_room(&room_id);
                if left_thread {
                    self.reset_conversation();
                }
                return;
            }
        };
        self.went_to_room(&room_id);
        self.effects.push(Effect::RoomOpened(room_id.clone()));
        self.previews.clear();
        self.sixel.clear();
        // Pictures of the room we are leaving are no longer needed.
        for (_, id) in self.terminal_images.drain() {
            self.effects
                .push(Effect::Terminal(crate::kitty::delete(id)));
        }
        self.composer_mentions.clear();
        self.viewport.follow_newest();
        self.notice = None;
        self.clear_selection();
        self.cancel_compose();
        // What was being written stays with its room.
        if let Some(previous) = previous {
            self.keep_draft(&previous);
        }
        self.composer.clear();
        if let Some(draft) = self.drafts.get(&room_id) {
            self.composer.set(&draft.clone());
        }
        self.reopen = None;
        self.state_dirty = true;
    }

    /// The cursor and the keys go to the room just opened (or again).
    fn went_to_room(&mut self, room_id: &str) {
        self.cursor_room = Some(room_id.to_owned());
        self.focus = Focus::Composer;
        self.filtering = false;
        self.filter.clear();
    }

    fn keep_draft(&mut self, room_id: &str) {
        let text = self.composer.text();
        if text.trim().is_empty() || matches!(self.compose, Compose::Edit { .. }) {
            self.drafts.remove(room_id);
        } else {
            self.drafts.insert(room_id.to_owned(), text.to_owned());
        }
    }

    /// The open room and the drafts, for the next start.
    pub fn session_state(&mut self) -> (Option<String>, HashMap<String, String>) {
        if let Some(open) = self.state.open_room.clone() {
            self.keep_draft(&open);
        }
        (self.state.open_room.clone(), self.drafts.clone())
    }

    /// Pick up where the last run stopped: its drafts, and its open room
    /// as soon as that is listed.
    pub fn restore_session(&mut self, last_room: Option<String>, drafts: HashMap<String, String>) {
        self.drafts = drafts;
        self.reopen = last_room.filter(|id| !id.is_empty());
    }

    /// Open a room outside a key press or click.
    pub(crate) fn open_later(&mut self, room: String) {
        let mut commands = Vec::new();
        self.open(room, &mut commands);
        self.effects
            .extend(commands.into_iter().map(Effect::Command));
    }

    /// The Matrix id meant by what was typed after `/dm` or `/invite`: an
    /// id as it is, or the name of someone in the open room (as the
    /// @-completion writes it).
    fn user_id_for(&self, typed: &str) -> String {
        let typed = typed.trim();
        if typed.starts_with('@') && typed.contains(':') {
            return typed.to_owned();
        }
        let name = typed.trim_start_matches('@').to_lowercase();
        self.state
            .members
            .iter()
            .find(|member| member.name.to_lowercase() == name)
            .map(|member| member.user_id.clone())
            .unwrap_or_else(|| typed.to_owned())
    }

    /// The rooms a message can be forwarded to, of every account (the open
    /// room's first), narrowed by what was typed.
    pub fn forward_targets(&self) -> Vec<&RoomEntry> {
        let Some(forward) = self.overlays.get::<Forward>() else {
            return Vec::new();
        };
        let account = self.active_account();
        let needle = forward.filter.text().trim().to_lowercase();
        let mut targets: Vec<&RoomEntry> = self
            .state
            .rooms
            .iter()
            .filter(|room| !room.is_space && !room.is_invite)
            .filter(|room| needle.is_empty() || room.name.to_lowercase().contains(&needle))
            .collect();
        targets.sort_by_key(|room| room.account != account);
        targets
    }

    fn forward_to(&mut self, room_id: String, commands: &mut Vec<Command>) {
        if let Some(forward) = self.overlays.take::<Forward>() {
            commands.push(Command::Forward {
                event_id: forward.event_id,
                room_id,
            });
            self.clear_selection();
            self.focus = Focus::Composer;
        }
    }

    fn on_forward_key(&mut self, key: KeyEvent, commands: &mut Vec<Command>) {
        let count = self.forward_targets().len();
        let Some(forward) = self.overlays.get_mut::<Forward>() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.overlays.close::<Forward>(),
            KeyCode::Enter => {
                let index = forward.cursor;
                if let Some(room) = self.forward_targets().get(index) {
                    let room_id = room.id.clone();
                    self.forward_to(room_id, commands);
                }
            }
            KeyCode::Up => forward.cursor = forward.cursor.saturating_sub(1),
            KeyCode::Down if count > 0 => forward.cursor = (forward.cursor + 1).min(count - 1),
            _ => {
                if forward.filter.edit(key) {
                    forward.cursor = 0;
                }
            }
        }
    }

    /// Ask for previews of the links in the newest messages that have not
    /// been asked about yet (when the setting is on).
    pub(crate) fn ask_link_previews(&mut self) {
        if !self.state.prefs.link_previews {
            return;
        }
        let wanted: Vec<String> = self
            .state
            .items
            .iter()
            .rev()
            .take(40)
            .filter(|item| matches!(item.kind, ItemKind::Text | ItemKind::Notice))
            .filter_map(|item| crate::inline::parse_links(&item.body).1.into_iter().next())
            .filter(|url| url.starts_with("http"))
            .collect();
        for url in wanted {
            if self.state.link_previews_asked.insert(url.clone()) {
                self.send_later(Command::PreviewLink(url));
            }
        }
    }

    /// Whether a dialog covers the conversation.
    pub fn dialog_open(&self) -> bool {
        !self.overlays.is_empty()
    }

    /// Select the first message after the "new messages" line.
    fn jump_to_unread(&mut self) {
        let after_marker = self
            .state
            .items
            .iter()
            .skip_while(|item| item.kind != ItemKind::ReadMarker)
            .find(|item| Self::selectable(item))
            .map(|item| item.event_id.clone());
        match after_marker {
            Some(event_id) => {
                self.select(event_id);
                self.viewport.reveal_selection();
                self.focus = Focus::Timeline;
            }
            None => self.notice = Some("Nothing new in this room.".to_owned()),
        }
    }

    /// Forget what the conversation view shows, before it shows something
    /// else of the same room.
    fn reset_conversation(&mut self) {
        self.state.items.clear();
        self.previews.clear();
        self.sixel.clear();
        for (_, id) in self.terminal_images.drain() {
            self.effects
                .push(Effect::Terminal(crate::kitty::delete(id)));
        }
        self.viewport.follow_newest();
        self.clear_selection();
        self.cancel_compose();
        self.focus = Focus::Composer;
    }

    /// Read (and write in) the thread that starts at this message.
    pub(crate) fn enter_thread(&mut self, event_id: String, commands: &mut Vec<Command>) {
        if self.state.enter_thread(&event_id) {
            self.reset_conversation();
            commands.push(Command::OpenThread { event_id });
        }
    }

    /// Back from a thread to the room's conversation.
    pub(crate) fn leave_thread(&mut self, commands: &mut Vec<Command>) {
        if self.state.leave_thread() {
            self.reset_conversation();
            commands.push(Command::CloseThread);
        }
    }

    /// Open the room before/after the open one in sidebar order.
    fn open_neighbour(&mut self, delta: isize, commands: &mut Vec<Command>) {
        let ids = self.visible_room_ids();
        if ids.is_empty() {
            return;
        }
        let current = self
            .state
            .open_room
            .as_ref()
            .and_then(|id| ids.iter().position(|candidate| candidate == id));
        let next = match current {
            Some(index) => (index as isize + delta).rem_euclid(ids.len() as isize) as usize,
            None => 0,
        };
        self.open(ids[next].clone(), commands);
    }

    fn toggle_pin(&mut self, room_id: &str, commands: &mut Vec<Command>) {
        if let Some(room) = self.state.rooms.iter_mut().find(|room| room.id == room_id) {
            // Show the change right away; the server confirms on next sync.
            room.favourite = !room.favourite;
            commands.push(Command::SetFavourite {
                room_id: room_id.to_owned(),
                value: room.favourite,
            });
        }
    }

    fn toggle_low_priority(&mut self, room_id: &str, commands: &mut Vec<Command>) {
        if let Some(room) = self.state.rooms.iter_mut().find(|room| room.id == room_id) {
            room.low_priority = !room.low_priority;
            commands.push(Command::SetLowPriority {
                room_id: room_id.to_owned(),
                value: room.low_priority,
            });
        }
    }

    fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.state.config_dirty = true;
    }

    fn next_sort(&mut self) {
        self.state.prefs.sort = self.state.prefs.sort.next();
        self.state.config_dirty = true;
        self.sidebar_scroll = 0;
    }

    // -- timeline -----------------------------------------------------------

    fn scroll_by(&mut self, delta: isize, commands: &mut Vec<Command>) {
        // Scrolling past the oldest loaded line asks the server for more.
        if self.viewport.scroll_by(delta) && self.state.open_room.is_some() {
            commands.push(Command::Paginate);
        }
    }

    fn selectable(item: &Item) -> bool {
        !item.event_id.is_empty()
            && !matches!(
                item.kind,
                ItemKind::State | ItemKind::DateDivider | ItemKind::ReadMarker
            )
    }

    fn clear_selection(&mut self) {
        self.selected = None;
        self.bar = Bar::Actions;
    }

    fn select(&mut self, event_id: String) {
        self.selected = Some(event_id);
        self.bar = Bar::Actions;
        self.focus = Focus::Timeline;
    }

    /// Move the selection to the previous/next message.
    fn move_selection(&mut self, delta: isize, commands: &mut Vec<Command>) {
        let ids: Vec<&str> = self
            .state
            .items
            .iter()
            .filter(|item| Self::selectable(item))
            .map(|item| item.event_id.as_str())
            .collect();
        if ids.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_deref()
            .and_then(|id| ids.iter().position(|candidate| *candidate == id));
        let next = match current {
            Some(0) if delta < 0 => {
                // Already on the oldest loaded message: fetch more.
                commands.push(Command::Paginate);
                0
            }
            Some(index) => (index as isize + delta).clamp(0, ids.len() as isize - 1) as usize,
            None => ids.len() - 1,
        };
        let id = ids[next].to_owned();
        self.select(id);
        self.viewport.reveal_selection();
    }

    fn on_bar(&mut self, action: BarAction, commands: &mut Vec<Command>) {
        let Some(item) = self.selected_item().cloned() else {
            self.clear_selection();
            return;
        };
        // Keys and old clicks reach here for every message: only what the
        // strip offers for this one goes through.
        let offered = self.message_actions(&item);
        let message_action = !matches!(
            action,
            BarAction::Emoji(_)
                | BarAction::MoreEmoji
                | BarAction::ConfirmDelete
                | BarAction::ConfirmEndPoll
                | BarAction::Cancel
        );
        if message_action && !offered.iter().any(|offer| offer.action == action) {
            return;
        }
        match action {
            BarAction::Reply => {
                let snippet = item
                    .body
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                self.compose = Compose::Reply {
                    event_id: item.event_id,
                    label: format!("{}: {snippet}", item.sender_name),
                };
                self.clear_selection();
                self.focus = Focus::Composer;
            }
            BarAction::React => self.bar = Bar::Emoji,
            BarAction::MoreEmoji => self.open_emoji_picker(PickFor::React(item.event_id)),
            BarAction::Emoji(key) => {
                commands.push(Command::React {
                    event_id: item.event_id,
                    key,
                });
                self.bar = Bar::Actions;
            }
            BarAction::Edit => {
                self.composer.set(&item.body);
                self.compose = Compose::Edit {
                    event_id: item.event_id,
                };
                self.clear_selection();
                self.focus = Focus::Composer;
            }
            BarAction::Delete => {
                self.bar = Bar::ConfirmDelete;
            }
            BarAction::ConfirmDelete if self.bar == Bar::ConfirmDelete => {
                commands.push(Command::Delete {
                    event_id: item.event_id,
                });
                self.clear_selection();
            }
            BarAction::ConfirmEndPoll if self.bar == Bar::ConfirmEndPoll => {
                let format = match &item.kind {
                    ItemKind::Poll(poll) => poll.format,
                    _ => PollFormat::default(),
                };
                commands.push(Command::EndPoll {
                    poll_event_id: item.event_id,
                    format,
                });
                self.clear_selection();
            }
            BarAction::CopyCode => {
                self.copy_code(&item.event_id, None);
                self.clear_selection();
                self.focus = Focus::Composer;
            }
            BarAction::Expand => self.toggle_expand(&item.event_id),
            BarAction::Thread => self.enter_thread(item.event_id.clone(), commands),
            BarAction::Pin => commands.push(Command::TogglePin {
                event_id: item.event_id.clone(),
            }),
            BarAction::Forward => {
                self.overlays.open(Forward {
                    event_id: item.event_id.clone(),
                    filter: Input::default(),
                    cursor: 0,
                });
            }
            BarAction::Copy => {
                self.effects.push(Effect::Copy(copy_text(&item)));
                self.notice = Some("Copied to the clipboard.".to_owned());
                self.clear_selection();
                self.focus = Focus::Composer;
            }
            BarAction::Open if matches!(item.kind, ItemKind::Media) => {
                self.notice = Some("Downloading…".to_owned());
                commands.push(Command::OpenMedia {
                    event_id: item.event_id,
                });
            }
            // No attachment: the first link of the message.
            BarAction::Open => {
                if let Some(address) = crate::inline::parse_links(&item.body).1.into_iter().next() {
                    commands.push(Command::OpenLink(address));
                }
            }
            BarAction::Cancel => {
                if self.bar == Bar::Actions {
                    self.clear_selection();
                    self.focus = Focus::Composer;
                } else {
                    self.bar = Bar::Actions;
                }
            }
            _ => {}
        }
    }

    // -- device verification -------------------------------------------------

    fn on_verify_input(&mut self, input: VerifyInput, commands: &mut Vec<Command>) {
        // A finished dialog just closes; the backend has nothing to tell.
        if matches!(
            self.overlays.get::<VerifyState>(),
            Some(VerifyState::Done | VerifyState::Failed(_))
        ) {
            self.overlays.close::<VerifyState>();
        }
        commands.push(Command::Verify(input));
    }

    fn on_verify_key(&mut self, key: KeyEvent, commands: &mut Vec<Command>) {
        let input = match (self.overlays.get::<VerifyState>(), key.code) {
            (Some(VerifyState::Incoming { .. }), KeyCode::Enter) => VerifyInput::Accept,
            (Some(VerifyState::Emojis(_)), KeyCode::Enter) => VerifyInput::Confirm,
            (Some(VerifyState::Done | VerifyState::Failed(_)), KeyCode::Enter) => {
                VerifyInput::Cancel
            }
            (Some(_), KeyCode::Esc) => VerifyInput::Cancel,
            _ => return,
        };
        self.on_verify_input(input, commands);
    }

    fn cancel_compose(&mut self) {
        if matches!(self.compose, Compose::Edit { .. }) {
            self.composer.clear();
        }
        self.compose = Compose::Message;
    }

    // -- backend updates ----------------------------------------------------

    /// Have the logo of the start screen ready as a picture, made for how
    /// this terminal draws pictures (again when that changed). Nothing
    /// when pictures are off: the start screen shows the text moth then.
    pub fn ensure_logo(&mut self) {
        let now = (self.graphics, self.cell_px);
        if self.graphics == Graphics::Off || self.logo_for == Some(now) {
            return;
        }
        self.logo_for = Some(now);
        self.previews.remove(crate::logo::KEY);
        self.sixel.remove(crate::logo::KEY);
        self.terminal_images.remove(crate::logo::KEY);
        if let Some(preview) = crate::logo::preview(self.graphics == Graphics::Kitty) {
            self.on_update(Update::Preview {
                event_id: crate::logo::KEY.to_owned(),
                preview,
            });
        }
    }

    pub fn on_update(&mut self, update: Update) {
        let update = match self.state.take(update) {
            Taken::Rooms { news, show } => {
                for news in news {
                    self.effects.push(Effect::Notify {
                        room: news.room,
                        title: news.title,
                    });
                }
                self.keep_cursor_listed();
                // A menu for a room that is gone has nothing left to offer.
                if self.room_menu_rows().is_empty() {
                    self.overlays.close::<RoomMenu>();
                }
                // A room we were told to go to; else back into the one
                // that was open last time.
                let reopen = self.reopen.clone().filter(|id| {
                    self.state.open_room.is_none()
                        && self.state.rooms.iter().any(|room| room.id == *id)
                });
                if let Some(room) = show.or(reopen) {
                    self.open_later(room);
                }
                return;
            }
            Taken::Show(room) => return self.open_later(room),
            Taken::Items => {
                // The selected message may have been deleted meanwhile.
                if self.selected.is_some() && self.selected_item().is_none() {
                    self.clear_selection();
                }
                self.try_reveal();
                self.ask_link_previews();
                return;
            }
            Taken::LeftThread => return self.reset_conversation(),
            Taken::Typing | Taken::Members | Taken::LinkPreview(_) | Taken::Nothing => return,
            Taken::Other(update) => update,
        };
        match update {
            // Taken by the client state, above.
            Update::Rooms(_)
            | Update::GoTo(_)
            | Update::Timeline { .. }
            | Update::Thread { .. }
            | Update::ThreadFailed { .. }
            | Update::Typing { .. }
            | Update::Members { .. }
            | Update::LinkPreview { .. } => {}
            Update::SearchResults { query, hits } => self.on_search_results(query, hits),
            Update::Reveal(event_id) => {
                self.pending_reveal = Some(event_id);
                self.try_reveal();
            }
            // Only for apps that show attachments themselves (iOS); the
            // terminal opens them with the desktop's viewer.
            Update::MediaFile { .. } => {}
            // Taken by `Accounts::deliver`: into the sections, on to the
            // account of the room a message is forwarded to.
            Update::Sections(_) | Update::ToForward { .. } => {}
            Update::RoomDetails(details) => self.on_room_details(details),
            Update::RecoveryKey(key) => {
                self.notice = None;
                self.overlays.open(RecoveryKey(key));
            }
            Update::Preview {
                event_id,
                mut preview,
            } => {
                if self.graphics == Graphics::Off {
                    return;
                }
                if self.graphics == Graphics::Sixel
                    && !preview.png.is_empty()
                    && let Some(sixel) = crate::sixel::for_preview(
                        &preview.png,
                        preview.png_columns,
                        preview.png_rows,
                        self.cell_px,
                    )
                {
                    self.sixel.insert(event_id.clone(), sixel);
                }
                if self.graphics == Graphics::Kitty && !preview.png.is_empty() {
                    let id = self.next_image_id;
                    self.next_image_id += 1;
                    self.effects.push(Effect::Terminal(crate::kitty::transmit(
                        id,
                        &preview.png,
                        preview.png_columns,
                        preview.png_rows,
                    )));
                    self.terminal_images.insert(event_id.clone(), id);
                }
                // The terminal has its copy now (or does not want one).
                preview.png = Vec::new();
                self.previews.insert(event_id, preview);
            }
            Update::Verify(Some(state)) => self.overlays.open(state),
            Update::Verify(None) => self.overlays.close::<VerifyState>(),
            // Without several accounts (see `accounts::Accounts::deliver`)
            // these are about the only one.
            Update::Verified(verified) => self.state.accounts[0].verified = Some(verified),
            Update::Sync(status) => self.state.accounts[0].sync = status,
            Update::Notice(text) => self.notice = Some(text),
            Update::OwnName(name) => self.state.accounts[0].own_name = Some(name),
            Update::PastedText(text) => {
                self.on_event(Event::Paste(text));
            }
            Update::PastedImage(path) => {
                if self.state.open_room.is_none() {
                    self.notice = Some("Open a room first.".to_owned());
                } else if self.composer.is_empty() {
                    self.composer.set(&format!("/file {path}"));
                    self.focus = Focus::Composer;
                    self.notice =
                        Some("Press Enter to send this picture, Ctrl+U to drop it.".to_owned());
                } else {
                    self.notice = Some(
                        "Send or clear your message first, then paste the picture.".to_owned(),
                    );
                }
            }
        }
    }

    // -- terminal events ----------------------------------------------------

    pub fn on_event(&mut self, event: Event) -> Vec<Command> {
        let mut commands = Vec::new();
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key, &mut commands),
            Event::Mouse(mouse) => self.on_mouse(mouse, &mut commands),
            Event::Paste(text) if self.overlay_paste(&text) => {}
            Event::Paste(text) => {
                self.focus = Focus::Composer;
                // Dropping a file onto the terminal pastes its path.
                match dropped_file(&text) {
                    Some(path) if self.composer.is_empty() => {
                        self.composer.set(&format!("/file {path}"));
                        self.notice = Some("Press Enter to send this file.".to_owned());
                    }
                    // Keep the line breaks: a pasted code block stays a block.
                    _ => {
                        let text = text.replace("\r\n", "\n").replace('\r', "\n");
                        self.composer.insert_str(text.trim_end_matches('\n'));
                    }
                }
            }
            Event::FocusGained => self.state.focused = true,
            Event::FocusLost => self.state.focused = false,
            _ => {}
        }
        self.refresh_spelling();
        self.tell_typing();
        for command in &commands {
            if let Command::React { key, .. } = command {
                self.used_emoji(key.clone());
            }
        }
        commands
    }

    /// The emoji on the reaction bar: the user's favourites.
    pub fn quick_reactions(&self) -> Vec<String> {
        self.emoji_usage.favourites(9)
    }

    /// The user took an emoji: it moves up in the reaction bar and `:`.
    pub(crate) fn used_emoji(&mut self, emoji: String) {
        self.emoji_usage.note(&emoji);
        self.effects.push(Effect::UsedEmoji(emoji));
    }

    /// Let the room know when we start or stop writing a message (not a
    /// command). Goes out with [`App::take_commands`].
    fn tell_typing(&mut self) {
        let text = self.composer.text();
        let writing = self.state.prefs.typing_notices
            && self.state.open_room.is_some()
            && !text.trim().is_empty()
            && !text.starts_with('/');
        let now = if writing { text } else { "" };
        if now == self.typing_told {
            return;
        }
        // Again with every change while writing, so that it does not time
        // out; once when it ends.
        let was_writing = !self.typing_told.is_empty();
        self.typing_told = now.to_owned();
        if writing || was_writing {
            self.send_later(Command::Typing(writing));
        }
    }

    fn on_key(&mut self, key: KeyEvent, commands: &mut Vec<Command>) {
        // A key bound in the config stands for the built-in one.
        let key = crate::keys::translate(&self.keymap, key);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        // An open dialog or popup gets the keys before anything else
        // (quitting still works).
        let quitting = ctrl && matches!(key.code, KeyCode::Char('q' | 'c'));
        // The help opens over any dialog, and closes again.
        if key.code == KeyCode::F(1) {
            return self.toggle_help();
        }
        if !quitting && self.overlay_key(key, commands) {
            return;
        }
        // `?` is the help, except where it is typed.
        if key.code == KeyCode::Char('?') && self.focus != Focus::Composer && !self.filtering {
            return self.toggle_help();
        }
        if self.focus == Focus::Composer && !ctrl && !alt && self.on_mention_key(key) {
            return;
        }

        // Global shortcuts first.
        match key.code {
            KeyCode::Char('q' | 'c') if ctrl => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('k') if ctrl => {
                self.focus = Focus::Sidebar;
                self.filtering = true;
                self.filter.clear();
                return;
            }
            KeyCode::Char('t') if ctrl => {
                self.set_theme(theme::next_after(self.theme.name));
                return;
            }
            KeyCode::Char('f') if ctrl => return self.open_search(""),
            // Terminals paste only text themselves; ask the clipboard, which
            // may hold a picture.
            KeyCode::Char('v') if ctrl => {
                commands.push(Command::ClipboardPaste);
                return;
            }
            KeyCode::F(2) => {
                self.overlays.open(SettingsDialog::default());
                return;
            }
            KeyCode::F(3) => return self.next_account_filter(),
            KeyCode::F(4) => return self.toggle_unread_only(),
            KeyCode::F(5) => {
                self.open_emoji_picker(PickFor::Insert);
                return;
            }
            KeyCode::F(6) => return self.open_format_menu_at_composer(),
            KeyCode::Up if alt => return self.open_neighbour(-1, commands),
            KeyCode::Down if alt => return self.open_neighbour(1, commands),
            KeyCode::PageUp => {
                return self.scroll_by(self.viewport.page(), commands);
            }
            KeyCode::PageDown => {
                return self.scroll_by(-self.viewport.page(), commands);
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.filtering = false;
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Composer,
                    Focus::Composer | Focus::Timeline => Focus::Sidebar,
                };
                return;
            }
            _ => {}
        }

        match self.focus {
            Focus::Sidebar => self.on_sidebar_key(key, commands),
            Focus::Timeline => self.on_timeline_key(key, commands),
            Focus::Composer => self.on_composer_key(key, commands),
        }
    }

    fn on_sidebar_key(&mut self, key: KeyEvent, commands: &mut Vec<Command>) {
        match key.code {
            KeyCode::Up => self.move_cursor(-1),
            KeyCode::Down => self.move_cursor(1),
            KeyCode::Enter => {
                let ids = self.visible_room_ids();
                let target = self
                    .cursor_room
                    .clone()
                    .filter(|id| ids.contains(id))
                    .or_else(|| ids.first().cloned());
                if let Some(room_id) = target {
                    self.open(room_id, commands);
                }
            }
            KeyCode::Esc => {
                if self.filtering || !self.filter.is_empty() {
                    self.filtering = false;
                    self.filter.clear();
                } else if self.state.open_room.is_some() {
                    self.focus = Focus::Composer;
                }
            }
            _ if self.filtering && self.filter.edit(key) => self.snap_cursor_to_filter(),
            KeyCode::Char('/') => {
                self.filtering = true;
                self.filter.clear();
            }
            KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::Char('p') => {
                if let Some(room_id) = self.cursor_room.clone() {
                    self.toggle_pin(&room_id, commands);
                }
            }
            KeyCode::Char('l') => {
                if let Some(room_id) = self.cursor_room.clone() {
                    self.toggle_low_priority(&room_id, commands);
                }
            }
            KeyCode::Char('s') => self.next_sort(),
            KeyCode::Char('m') => self.toggle_mixed_list(),
            KeyCode::Char('a') => self.next_account_filter(),
            KeyCode::Char('U') => self.toggle_unread_only(),
            // Notifications of the room: all, mentions only, muted.
            KeyCode::Char('M') => {
                let cursor = self.cursor_room.clone();
                if let Some(room) = self
                    .state
                    .rooms
                    .iter_mut()
                    .find(|room| Some(&room.id) == cursor.as_ref())
                {
                    room.notify = room.notify.next();
                    commands.push(Command::SetNotify {
                        room_id: room.id.clone(),
                        mode: room.notify,
                    });
                    self.notice = Some(format!("{}: {}.", room.name, room.notify.label()));
                }
            }
            KeyCode::Char('i') => {
                if let Some(room_id) = self.cursor_room.clone() {
                    self.open_room_info(room_id, commands);
                }
            }
            KeyCode::Char('g') => self.open_section_picker(),
            KeyCode::Char('G') => self.open_section_manager(0),
            KeyCode::Char('z') => self.fold_cursor_section(),
            KeyCode::Char('Z') => {
                self.state.folded.clear();
                self.state.config_dirty = true;
            }
            _ => {}
        }
    }

    fn on_timeline_key(&mut self, key: KeyEvent, commands: &mut Vec<Command>) {
        if self.bar == Bar::Emoji
            && let KeyCode::Char(c @ '1'..='9') = key.code
            && let Some(key) = self.quick_reactions().get(c as usize - '1' as usize)
        {
            return self.on_bar(BarAction::Emoji(key.clone()), commands);
        }
        if self.bar == Bar::Emoji && key.code == KeyCode::Char('0') {
            return self.on_bar(BarAction::MoreEmoji, commands);
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1, commands),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1, commands),
            KeyCode::End | KeyCode::Char('G') => {
                self.viewport.follow_newest();
                self.clear_selection();
            }
            KeyCode::Esc => self.on_bar(BarAction::Cancel, commands),
            KeyCode::Char('i') => {
                self.clear_selection();
                self.focus = Focus::Composer;
            }
            KeyCode::Enter | KeyCode::Char('y') if self.bar == Bar::ConfirmDelete => {
                self.on_bar(BarAction::ConfirmDelete, commands);
            }
            KeyCode::Enter | KeyCode::Char('y') if self.bar == Bar::ConfirmEndPoll => {
                self.on_bar(BarAction::ConfirmEndPoll, commands);
            }
            KeyCode::Enter => self.on_bar(BarAction::Reply, commands),
            KeyCode::Char('u') => self.jump_to_unread(),
            KeyCode::Char(c) => {
                if let Some(action) = self.message_action_for_key(c) {
                    self.on_bar(action, commands);
                } else if let Some(notice) = Self::unavailable_notice(c)
                    && self.selected_item().is_some()
                {
                    self.notice = Some(notice.to_owned());
                }
            }
            _ => {}
        }
    }

    fn on_composer_key(&mut self, key: KeyEvent, commands: &mut Vec<Command>) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // Alt+Enter (or Shift+Enter where the terminal tells them apart)
            // starts a new line; plain Enter sends.
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.composer.insert('\n');
            }
            KeyCode::Enter => self.submit(commands),
            KeyCode::Esc => {
                if self.compose != Compose::Message {
                    self.cancel_compose();
                } else if self.state.open_thread.is_some() {
                    self.leave_thread(commands);
                } else {
                    self.focus = Focus::Sidebar;
                }
            }
            KeyCode::Backspace => {
                self.composer.backspace();
                self.mention_edited();
            }
            KeyCode::Delete => self.composer.delete(),
            KeyCode::Left => self.composer.left(),
            KeyCode::Right => self.composer.right(),
            KeyCode::Home => self.composer.home(),
            KeyCode::End => self.composer.end(),
            // Up moves within a message of several lines; from its first
            // line it walks into the conversation.
            KeyCode::Up => {
                if !self.composer.line_up() {
                    self.move_selection(-1, commands);
                }
            }
            KeyCode::Down => {
                self.composer.line_down();
            }
            KeyCode::Char('w') if ctrl => self.composer.delete_word(),
            KeyCode::Char('u') if ctrl => self.composer.clear(),
            KeyCode::Char('a') if ctrl => self.composer.home(),
            KeyCode::Char('e') if ctrl => self.composer.end(),
            KeyCode::Char(c) if !ctrl => {
                self.composer.insert(c);
                self.mention_edited();
            }
            _ => {}
        }
    }

    fn submit(&mut self, commands: &mut Vec<Command>) {
        // The pinned messages are for reading; a message typed there
        // would not land where it seems to.
        let pinned_view = self.state.pinned_view();
        if pinned_view && !self.composer.text().trim_start().starts_with('/') {
            self.notice = Some("Esc goes back to the room, where you can write.".to_owned());
            return;
        }
        let text = self.composer.take();
        let text = text.trim();
        if text.is_empty() {
            return;
        }

        if let Some(command) = text.strip_prefix('/')
            && !text.starts_with("/me ")
        {
            let (name, argument) = command.split_once(' ').unwrap_or((command, ""));
            match name {
                "quit" | "q" => self.should_quit = true,
                "backup" => commands.push(Command::Backup {
                    renew: argument.trim() == "new",
                }),
                "reset-identity" => self.overlays.open(ResetPrompt::default()),
                "stop" => commands.push(Command::StopAudio),
                "pins" if self.state.open_room.is_none() => {
                    self.notice = Some("Open a room first.".to_owned());
                }
                "pins" => {
                    self.enter_thread(crate::matrix::PINNED_VIEW.to_owned(), commands);
                }
                "info" | "invite" | "leave" if self.state.open_room.is_none() => {
                    self.notice = Some("Open a room first.".to_owned());
                }
                "info" => {
                    let room_id = self.state.open_room.clone().unwrap_or_default();
                    self.open_room_info(room_id, commands);
                }
                "leave" => {
                    let room_id = self.state.open_room.clone().unwrap_or_default();
                    self.open_room_info(room_id, commands);
                    if let Some(info) = self.overlays.get_mut::<RoomInfo>() {
                        info.mode = InfoMode::Confirm(InfoConfirm::Leave);
                    }
                }
                "invite" | "dm" | "room" | "join" if argument.trim().is_empty() => {
                    self.composer.set(&format!("/{name} "));
                    self.notice = Some(
                        match name {
                            "room" => "Give the new room a name: /room Book club",
                            "join" => "Which room? /join #name:server",
                            _ => "Who? Like @name:server",
                        }
                        .to_owned(),
                    );
                }
                "invite" => commands.push(Command::Invite {
                    room_id: self.state.open_room.clone().unwrap_or_default(),
                    user_id: self.user_id_for(argument),
                }),
                "dm" => commands.push(Command::StartDm {
                    user_id: self.user_id_for(argument),
                }),
                "room" => commands.push(Command::CreateRoom {
                    name: argument.trim().to_owned(),
                }),
                "join" => commands.push(Command::Join {
                    target: argument.trim().to_owned(),
                }),
                "theme" => match theme::by_name(argument) {
                    Some(theme) => self.set_theme(theme),
                    None => {
                        let names: Vec<&str> =
                            theme::all().iter().map(|theme| theme.name).collect();
                        self.notice = Some(format!("Themes: {}", names.join(", ")));
                    }
                },
                "poll" => match parse_poll(argument) {
                    Ok(_) if self.state.open_room.is_none() => {
                        self.notice = Some("Open a room first.".to_owned());
                    }
                    Ok((question, answers, multiple, hidden)) => {
                        self.viewport.follow_newest();
                        commands.push(Command::CreatePoll {
                            question,
                            answers,
                            multiple,
                            hidden,
                        });
                    }
                    Err(problem) => {
                        // Keep what was typed so it can be corrected.
                        self.composer.set(text);
                        self.notice = Some(problem);
                    }
                },
                "file" | "upload" if !argument.trim().is_empty() => {
                    if self.state.open_room.is_none() {
                        self.notice = Some("Open a room first.".to_owned());
                    } else {
                        self.viewport.follow_newest();
                        // "/file picture.png | what it shows"
                        let (path, caption) = match argument.split_once(" | ") {
                            Some((path, caption)) => (path, caption.trim()),
                            None => (argument, ""),
                        };
                        commands.push(Command::SendFile {
                            path: path.trim().to_owned(),
                            caption: (!caption.is_empty()).then(|| caption.to_owned()),
                        });
                    }
                }
                "paste" => {
                    if self.state.open_room.is_none() {
                        self.notice = Some("Open a room first.".to_owned());
                    } else {
                        self.viewport.follow_newest();
                        self.notice = Some("Sending the picture from the clipboard…".to_owned());
                        commands.push(Command::PasteImage);
                    }
                }
                "verify" if !argument.trim().is_empty() => commands.push(Command::VerifyUser {
                    user_id: self.user_id_for(argument),
                }),
                "verify" => commands.push(Command::VerifyStart),
                "settings" => self.overlays.open(SettingsDialog::default()),
                "help" => self.toggle_help(),
                "update" => self.update_command(),
                "accounts" => self.open_accounts(),
                "search" => {
                    self.open_search(argument.trim());
                    if !argument.trim().is_empty() {
                        self.on_search_key(
                            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                            commands,
                        );
                    }
                }
                "recover" if !argument.trim().is_empty() => {
                    self.notice = Some("Checking the recovery key…".to_owned());
                    commands.push(Command::Recover(argument.trim().to_owned()));
                }
                _ => {
                    self.notice = Some(
                        "Commands: /me, /poll Q | A | B [--multi] [--hidden], /file <path>, /paste, /search, /settings, /accounts, /info, /invite, /dm, /room, /join, /leave, /verify, /recover <key>, /theme, /quit"
                            .to_owned(),
                    );
                }
            }
            return;
        }

        let Some(room_id) = self.state.open_room.clone() else {
            self.notice = Some("Open a room first.".to_owned());
            return;
        };
        self.viewport.follow_newest();
        let text = text.to_owned();
        // Only mentions whose text survived the editing count.
        let mentions: Vec<Member> = std::mem::take(&mut self.composer_mentions)
            .into_iter()
            .filter(|member| text.contains(&format!("@{}", member.name)))
            .collect();
        match std::mem::replace(&mut self.compose, Compose::Message) {
            Compose::Message => commands.push(Command::Send {
                room_id,
                text,
                mentions,
            }),
            Compose::Reply { event_id, .. } => commands.push(Command::Reply {
                event_id,
                text,
                mentions: mentions.clone(),
            }),
            Compose::Edit { event_id } => commands.push(Command::Edit {
                event_id,
                text,
                mentions: mentions.clone(),
            }),
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent, commands: &mut Vec<Command>) {
        let position = Position::new(mouse.column, mouse.row);
        // Later regions sit on top of earlier ones.
        let hit_at = |hits: &[Hit]| {
            hits.iter()
                .rev()
                .find(|hit| hit.rect.contains(position))
                .map(|hit| hit.action.clone())
        };
        match mouse.kind {
            // The wheel scrolls the emoji picker, but does not reach past
            // an open dialog.
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if matches!(self.overlays.top(), Some(Overlay::EmojiPicker(_))) =>
            {
                let up = mouse.kind == MouseEventKind::ScrollUp;
                self.scroll_emoji_picker(if up { -3 } else { 3 });
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if matches!(self.overlays.top(), Some(Overlay::Help(_))) =>
            {
                if let Some(help) = self.overlays.get_mut::<Help>() {
                    help.scroll = match mouse.kind {
                        MouseEventKind::ScrollUp => help.scroll.saturating_sub(3),
                        _ => help.scroll + 3,
                    };
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if self.dialog_open() => {}
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = mouse.kind == MouseEventKind::ScrollUp;
                if self.sidebar_area.contains(position) {
                    self.sidebar_scroll = if up {
                        self.sidebar_scroll.saturating_sub(2)
                    } else {
                        // Clamped to the content by the next draw.
                        self.sidebar_scroll + 2
                    };
                } else if self.timeline_area.contains(position) {
                    self.scroll_by(if up { 3 } else { -3 }, commands);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let popup_hit = matches!(hit_at(&self.hits), Some(HitAction::MentionPick(_)));
                if !self.dialog_open() && !popup_hit && self.composer_text_area.contains(position) {
                    // Click into the text: put the cursor there.
                    let index = crate::ui::composer_index_at(
                        self.composer.text(),
                        self.composer_text_area.width,
                        self.composer_first_row + (position.y - self.composer_text_area.y) as usize,
                        position.x - self.composer_text_area.x,
                    );
                    self.composer.set_cursor(index);
                    self.mention_hidden = false;
                    self.focus = Focus::Composer;
                } else if let Some(action) = hit_at(&self.hits) {
                    self.on_hit(action, commands);
                }
            }
            // Right-click a room for its menu.
            MouseEventKind::Down(MouseButton::Right) => {
                match hit_at(&self.hits) {
                    Some(HitAction::OpenRoom(room_id)) => {
                        self.overlays.open(dialogs::RoomMenu {
                            room_id,
                            x: position.x,
                            y: position.y,
                            cursor: 0,
                        });
                    }
                    // Right-click one of your own sections to rename or move it.
                    Some(action @ HitAction::ToggleSection(_)) => {
                        if let Some(at) = self.own_section_of(&action) {
                            self.open_section_manager(at);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    pub fn on_hit(&mut self, action: HitAction, commands: &mut Vec<Command>) {
        match action {
            HitAction::OpenRoom(room_id) => self.open(room_id, commands),
            HitAction::FocusComposer => self.focus = Focus::Composer,
            HitAction::FocusTimeline => self.focus = Focus::Timeline,
            HitAction::OpenThread(event_id) => self.enter_thread(event_id, commands),
            HitAction::CloseThread => self.leave_thread(commands),
            HitAction::OpenLink(address) => commands.push(Command::OpenLink(address)),
            HitAction::Select(event_id) => {
                // A second click on the same message soon after: a
                // double-click, which opens a file or picture.
                let now = std::time::Instant::now();
                let double = self.last_click.take().is_some_and(|(id, at)| {
                    id == event_id && now.duration_since(at) <= DOUBLE_CLICK
                });
                if double {
                    self.select(event_id);
                    let media = self
                        .selected_item()
                        .is_some_and(|item| matches!(item.kind, ItemKind::Media));
                    if media {
                        self.on_bar(BarAction::Open, commands);
                    }
                    return;
                }
                self.last_click = Some((event_id.clone(), now));
                if self.selected.as_deref() == Some(event_id.as_str()) {
                    self.clear_selection();
                } else {
                    self.select(event_id);
                }
            }
            HitAction::JumpToBottom => self.viewport.follow_newest(),
            HitAction::NextTheme => self.set_theme(theme::next_after(self.theme.name)),
            HitAction::NextSort => self.next_sort(),
            HitAction::ToggleUnread => self.toggle_unread_only(),
            HitAction::ToggleSection(title) => self.toggle_section(&title),
            HitAction::AccountTab(filter) => self.set_account_filter(filter),
            HitAction::SectionPick(row) => self.pick_section(row),
            HitAction::SectionEdit(edit) => self.edit_section(edit),
            HitAction::RoomMenuPick(item) => self.pick_room_menu(item, commands),
            HitAction::CopyCode { event_id, index } => self.copy_code(&event_id, Some(index)),
            HitAction::ToggleExpand(event_id) => self.toggle_expand(&event_id),
            HitAction::PollTemplate => {
                self.composer.set("/poll Question? | Answer 1 | Answer 2");
                self.focus = Focus::Composer;
                self.notice = Some(
                    "Edit the question and answers, then press Enter. Add --multi or --hidden if wanted."
                        .to_owned(),
                );
            }
            HitAction::FileTemplate => {
                self.composer.set("/file ~/");
                self.mention_hidden = false;
                self.mention_selected = 0;
                self.focus = Focus::Composer;
                self.notice = Some(
                    "Type to narrow down, Tab to complete, Enter to send. /paste sends a copied picture."
                        .to_owned(),
                );
            }
            HitAction::OpenEmojiPicker => self.open_emoji_picker(PickFor::Insert),
            HitAction::OpenFormatMenu { x, y } => self.open_format_menu(x, y),
            HitAction::FormatPick(format) => self.apply_format(format),
            HitAction::EmojiPick(emoji) => self.pick_emoji(emoji, commands),
            HitAction::CancelCompose => self.cancel_compose(),
            HitAction::MentionPick(index) => {
                self.accept_suggestion(index);
                self.focus = Focus::Composer;
            }
            HitAction::Verify(input) => self.on_verify_input(input, commands),
            HitAction::Blocked => {}
            HitAction::OpenSettings => {
                self.overlays.close::<Search>();
                self.overlays.open(SettingsDialog::default());
            }
            HitAction::Accounts(action) => self.accounts_action(action),
            HitAction::RoomInfo(action) => self.room_info_action(action, commands),
            HitAction::ForwardTo(room_id) => self.forward_to(room_id, commands),
            HitAction::Setting(setting) => {
                if let Some(settings) = self.overlays.get_mut::<SettingsDialog>() {
                    settings.cursor = setting.index();
                }
                self.change_setting(setting, true);
            }
            HitAction::OpenSearch => self.open_search(""),
            HitAction::SearchScope(all_chats) => self.set_search_scope(all_chats),
            HitAction::SearchHit { room_id, event_id } => {
                self.open_hit(room_id, event_id, commands)
            }
            HitAction::CloseDialogs => self.overlays.click_outside(),
            HitAction::React { event_id, key } => commands.push(Command::React { event_id, key }),
            HitAction::AskEndPoll(event_id) => {
                self.select(event_id);
                self.bar = Bar::ConfirmEndPoll;
            }
            HitAction::Bar(action) => self.on_bar(action, commands),
            HitAction::Vote {
                poll_event_id,
                answer_id,
            } => {
                let poll = self.state.items.iter().find_map(|item| match &item.kind {
                    ItemKind::Poll(poll) if item.event_id == poll_event_id => Some(poll),
                    _ => None,
                });
                if let Some(poll) = poll
                    && let Some(answers) = next_selection(poll, &answer_id)
                {
                    commands.push(Command::Vote {
                        poll_event_id,
                        answers,
                        format: poll.format,
                    });
                }
            }
        }
    }
}

/// The selection to send after clicking `answer_id`, or `None` when the click
/// changes nothing (poll ended, same single choice, or the limit is reached).
pub fn next_selection(poll: &Poll, answer_id: &str) -> Option<Vec<String>> {
    if poll.is_ended {
        return None;
    }
    let mut selection: Vec<String> = poll
        .answers
        .iter()
        .filter(|answer| answer.self_voted)
        .map(|answer| answer.id.clone())
        .collect();
    let position = selection.iter().position(|id| id == answer_id);

    if poll.max_selections <= 1 {
        if position.is_some() {
            return None;
        }
        return Some(vec![answer_id.to_owned()]);
    }
    match position {
        Some(index) => {
            selection.remove(index);
        }
        None if selection.len() as u64 >= poll.max_selections => return None,
        None => selection.push(answer_id.to_owned()),
    }
    Some(selection)
}

/// What "copy" puts on the clipboard for a message: its text, and for a
/// poll the question with its answers.
pub fn copy_text(item: &Item) -> String {
    match &item.kind {
        ItemKind::Poll(poll) => {
            let mut text = poll.question.clone();
            for answer in &poll.answers {
                text.push_str("\n- ");
                text.push_str(&answer.text);
            }
            text
        }
        _ => item.body.clone(),
    }
}

/// If `text` is the path of an existing file (as terminals paste it when a
/// file is dropped onto them: possibly quoted, backslash-escaped or as a
/// `file://` address), return the cleaned path.
pub fn dropped_file(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.contains('\n') {
        return None;
    }
    let unquoted = text
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .or_else(|| {
            text.strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
        });
    let path = match (unquoted, text.strip_prefix("file://")) {
        (Some(inner), _) => inner.to_owned(),
        (None, Some(url_path)) => percent_decode(url_path),
        (None, None) => text.replace("\\ ", " "),
    };
    (path.starts_with('/') && std::path::Path::new(&path).is_file()).then_some(path)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = (bytes[index] == b'%')
            .then(|| text.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match hex {
            Some(byte) => {
                out.push(byte);
                index += 3;
            }
            None => {
                out.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse `/poll Question | A | B [--multi] [--hidden]` into
/// (question, answers, multiple choice, hidden results).
pub fn parse_poll(argument: &str) -> Result<(String, Vec<String>, bool, bool), String> {
    let mut multiple = false;
    let mut hidden = false;
    let words: Vec<&str> = argument
        .split_whitespace()
        .filter(|word| match *word {
            "--multi" => {
                multiple = true;
                false
            }
            "--hidden" => {
                hidden = true;
                false
            }
            _ => true,
        })
        .collect();
    let cleaned = words.join(" ");
    let mut parts = cleaned
        .split('|')
        .map(str::trim)
        .filter(|part| !part.is_empty());
    let usage = "Usage: /poll Question | Answer 1 | Answer 2 [--multi] [--hidden]";
    let question = parts.next().ok_or(usage)?.to_owned();
    let answers: Vec<String> = parts.map(str::to_owned).collect();
    if answers.len() < 2 {
        return Err(usage.to_owned());
    }
    if answers.len() > 20 {
        return Err("A poll can have at most 20 answers.".to_owned());
    }
    Ok((question, answers, multiple, hidden))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PollAnswer;
    use mothrix_core::prefs::SortMode;

    fn room(id: &str, name: &str, is_dm: bool) -> RoomEntry {
        RoomEntry {
            id: id.into(),
            name: name.into(),
            is_dm,
            ..RoomEntry::default()
        }
    }

    fn app() -> App {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.on_update(Update::Rooms(vec![
            room("!a", "Lunch Club", false),
            room("!b", "Amy Winter", true),
            room("!c", "Dev Chat", false),
        ]));
        app
    }

    fn message(event_id: &str, body: &str, own: bool) -> Item {
        Item {
            key: event_id.into(),
            event_id: event_id.into(),
            sender_name: "Amy".into(),
            body: body.into(),
            is_own: own,
            can_edit: own,
            can_delete: own,
            ..Item::default()
        }
    }

    /// An app with room `!a` open and three messages loaded.
    fn chat() -> App {
        let mut app = app();
        app.on_event(key(KeyCode::Enter));
        app.on_update(Update::Timeline {
            room_id: "!a".into(),
            items: vec![
                message("$1", "first", false),
                message("$2", "mine", true),
                message("$3", "last", false),
            ],
        });
        app
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_event(key(KeyCode::Char(c)));
        }
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    /// The usual three rooms, plus a second account "work" with two.
    fn two_accounts() -> App {
        let mut app = app();
        let mut work = app.state.accounts[0].clone();
        work.profile = "work".into();
        work.label = "work".into();
        work.user_id = "@me:work".into();
        app.state.accounts.push(work);
        let mut rooms = app.state.rooms.clone();
        for (id, name, is_dm) in [("work/!a", "Standup", false), ("work/!z", "Boss", true)] {
            rooms.push(RoomEntry {
                account: 1,
                unread: 2,
                ..room(id, name, is_dm)
            });
        }
        app.on_update(Update::Rooms(rooms));
        app
    }

    /// Click the sidebar line that [`row_names`] calls `name`.
    fn click_row(app: &mut App, name: &str) {
        let at = row_names(app).iter().position(|row| row == name);
        let action = app.sidebar_lines()[at.expect(name)].action.clone();
        app.on_hit(action, &mut Vec::new());
    }

    fn row_names(app: &App) -> Vec<String> {
        app.sidebar_rows()
            .into_iter()
            .map(|row| match row {
                SidebarRow::Account { index } => format!("[{}]", app.state.accounts[index].profile),
                SidebarRow::Header { title } => title,
                SidebarRow::Folded { title, .. } => format!("{title} (folded)"),
                SidebarRow::Room { index } => app.state.rooms[index].name.clone(),
            })
            .collect()
    }

    #[test]
    fn several_accounts_are_listed_one_under_the_other() {
        let app = two_accounts();
        assert_eq!(
            row_names(&app),
            [
                "[default]",
                "ROOMS",
                "Lunch Club",
                "Dev Chat",
                "PEOPLE",
                "Amy Winter",
                "[work]",
                "ROOMS",
                "Standup",
                "PEOPLE",
                "Boss"
            ]
        );
    }

    #[test]
    fn a_cycles_through_the_accounts_and_back_to_all() {
        let mut app = two_accounts();
        app.focus = Focus::Sidebar;
        app.on_event(key(KeyCode::Char('a')));
        assert_eq!(app.state.account_filter, Some(0));
        assert_eq!(
            row_names(&app),
            ["ROOMS", "Lunch Club", "Dev Chat", "PEOPLE", "Amy Winter"]
        );
        app.on_event(key(KeyCode::Char('a')));
        assert_eq!(row_names(&app), ["ROOMS", "Standup", "PEOPLE", "Boss"]);
        // The cursor moved along to a room that is listed.
        assert_eq!(app.cursor_room.as_deref(), Some("work/!a"));
        assert_eq!(app.active_account(), 1);
        app.on_event(Event::Key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE)));
        assert_eq!(app.state.account_filter, None);
        assert!(app.state.config_dirty);
    }

    #[test]
    fn folding_a_section_of_one_account_leaves_the_others_open() {
        let mut app = two_accounts();
        // The cursor is in the first account's ROOMS.
        app.cursor_room = Some("!a".into());
        app.focus = Focus::Sidebar;
        app.on_event(key(KeyCode::Char('z')));
        assert_eq!(
            row_names(&app),
            [
                "[default]",
                "ROOMS (folded)",
                "PEOPLE",
                "Amy Winter",
                "[work]",
                "ROOMS",
                "Standup",
                "PEOPLE",
                "Boss"
            ]
        );
        // The same holds while only that account is shown.
        app.on_hit(HitAction::AccountTab(Some(0)), &mut Vec::new());
        assert_eq!(row_names(&app), ["ROOMS (folded)", "PEOPLE", "Amy Winter"]);
        app.on_hit(HitAction::AccountTab(Some(1)), &mut Vec::new());
        assert_eq!(row_names(&app), ["ROOMS", "Standup", "PEOPLE", "Boss"]);
    }

    #[test]
    fn the_unread_view_lists_only_unread_chats_of_all_accounts() {
        let mut app = two_accounts();
        let mut rooms = app.state.rooms.clone();
        rooms[1].mentions = 1; // Amy Winter (default account)
        rooms[1].unread = 1;
        app.on_update(Update::Rooms(rooms));
        app.focus = Focus::Sidebar;
        app.on_event(key(KeyCode::Char('U')));
        // Mentions first; then the unread rooms of the other account.
        assert_eq!(row_names(&app), ["UNREAD", "Amy Winter", "Standup", "Boss"]);

        // The open room stays listed after it was read.
        let mut commands = Vec::new();
        app.on_hit(HitAction::OpenRoom("work/!a".into()), &mut commands);
        let mut rooms = app.state.rooms.clone();
        for room in &mut rooms {
            if room.id == "work/!a" {
                room.unread = 0;
            }
        }
        app.on_update(Update::Rooms(rooms));
        assert_eq!(row_names(&app), ["UNREAD", "Amy Winter", "Standup", "Boss"]);
        app.on_hit(HitAction::OpenRoom("work/!z".into()), &mut commands);
        assert_eq!(row_names(&app), ["UNREAD", "Amy Winter", "Boss"]);

        // F4 leads back to everything.
        app.on_event(Event::Key(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE)));
        assert!(row_names(&app).contains(&"Lunch Club".to_owned()));
    }

    #[test]
    fn an_account_folds_like_a_section() {
        let mut app = two_accounts();
        click_row(&mut app, "[default]");
        assert_eq!(
            row_names(&app),
            ["[default]", "[work]", "ROOMS", "Standup", "PEOPLE", "Boss"]
        );
        // Filtering looks inside, and leaves out accounts without a match.
        app.filter.set("lunch");
        assert_eq!(row_names(&app), ["[default]", "ROOMS", "Lunch Club"]);
    }

    #[test]
    fn commands_without_a_room_follow_the_open_room() {
        let mut app = two_accounts();
        assert_eq!(app.active_account(), 0);
        let mut commands = Vec::new();
        app.on_hit(HitAction::OpenRoom("work/!a".into()), &mut commands);
        assert!(matches!(&commands[0], Command::Open(id) if id == "work/!a"));
        assert_eq!(app.active_account(), 1);
        assert_eq!(app.account().user_id, "@me:work");
        // Also while only the other account is listed.
        app.on_hit(HitAction::AccountTab(Some(0)), &mut commands);
        assert_eq!(app.active_account(), 1);
    }

    #[test]
    fn one_account_has_no_account_lines_and_a_does_nothing() {
        let mut app = app();
        app.focus = Focus::Sidebar;
        app.on_event(key(KeyCode::Char('a')));
        assert_eq!(app.state.account_filter, None);
        assert!(!row_names(&app).iter().any(|row| row.starts_with('[')));
    }

    #[test]
    fn rooms_of_a_space_are_listed_under_it_and_spaces_themselves_are_not() {
        let mut app = app();
        let mut rooms = app.state.rooms.clone();
        rooms[0].space = "School".into(); // Lunch Club
        rooms[2].space = "School/Class 7b".into(); // Dev Chat
        rooms.push(RoomEntry {
            is_space: true,
            ..room("!s", "School", false)
        });
        app.on_update(Update::Rooms(rooms));
        assert_eq!(
            row_names(&app),
            [
                "School",
                "Lunch Club",
                "School/Class 7b",
                "Dev Chat",
                "PEOPLE",
                "Amy Winter"
            ]
        );
        // Folding the space takes its subspace along.
        click_row(&mut app, "School");
        assert_eq!(row_names(&app), ["School (folded)", "PEOPLE", "Amy Winter"]);
        app.state.folded.clear();

        // An own section (mixed list) wins over the space.
        app.state.prefs.mixed_list = true;
        app.state.sections = crate::sections::OwnSections::new(vec![crate::model::Section {
            name: "Mine".into(),
            rooms: vec!["!a".into()],
        }]);
        assert_eq!(
            row_names(&app),
            [
                "Mine",
                "Lunch Club",
                "School",
                "School/Class 7b",
                "Dev Chat",
                "CHATS",
                "Amy Winter"
            ]
        );

        // Switched off, spaces play no part.
        app.state.prefs.mixed_list = false;
        app.state.prefs.spaces = false;
        assert_eq!(
            row_names(&app),
            ["ROOMS", "Lunch Club", "Dev Chat", "PEOPLE", "Amy Winter"]
        );
    }

    #[test]
    fn what_was_typed_stays_with_its_room() {
        let mut app = chat();
        let first = app.state.open_room.clone().unwrap();
        type_text(&mut app, "half a thought");
        let other = app
            .state
            .rooms
            .iter()
            .find(|room| room.id != first)
            .unwrap()
            .id
            .clone();
        app.on_hit(HitAction::OpenRoom(other.clone()), &mut Vec::new());
        assert!(app.composer.is_empty());
        type_text(&mut app, "something else");
        app.on_hit(HitAction::OpenRoom(first.clone()), &mut Vec::new());
        assert_eq!(app.composer.text(), "half a thought");

        // Sent: nothing is left over.
        app.on_event(key(KeyCode::Enter));
        let (open, drafts) = app.session_state();
        assert_eq!(open, Some(first));
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[&other], "something else");
    }

    #[test]
    fn the_room_open_last_time_is_opened_again_once_it_is_listed() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.restore_session(
            Some("!c".into()),
            HashMap::from([("!c".to_owned(), "draft".to_owned())]),
        );
        app.on_update(Update::Rooms(vec![room("!a", "Lunch Club", false)]));
        assert!(app.take_commands().is_empty());
        app.on_update(Update::Rooms(vec![
            room("!a", "Lunch Club", false),
            room("!c", "Dev Chat", false),
        ]));
        assert!(matches!(&app.take_commands()[..], [Command::Open(id)] if id == "!c"));
        assert_eq!(app.composer.text(), "draft");
    }

    #[test]
    fn u_jumps_to_the_first_new_message() {
        let mut app = chat();
        let mut items = vec![message("$old", "seen", false)];
        items.push(Item {
            key: "marker".into(),
            kind: ItemKind::ReadMarker,
            ..Item::default()
        });
        items.push(message("$new1", "first new", false));
        items.push(message("$new2", "second new", false));
        let room_id = app.state.open_room.clone().unwrap();
        app.on_update(Update::Timeline { room_id, items });
        app.focus = Focus::Timeline;
        app.on_event(key(KeyCode::Char('u')));
        assert_eq!(app.selected.as_deref(), Some("$new1"));
    }

    #[test]
    fn the_room_hears_when_we_write_and_when_we_stop() {
        let mut app = chat();
        type_text(&mut app, "hi");
        let told = app.take_commands();
        assert!(
            matches!(&told[..], [Command::Typing(true), Command::Typing(true)]),
            "{told:?}"
        );
        app.on_event(key(KeyCode::Enter));
        assert!(matches!(&app.take_commands()[..], [Command::Typing(false)]));

        // Commands are not messages being written; and it can be switched off.
        type_text(&mut app, "/se");
        assert!(app.take_commands().is_empty());
        app.composer.clear();
        app.state.prefs.typing_notices = false;
        type_text(&mut app, "quiet");
        assert!(app.take_commands().is_empty());
    }

    #[test]
    fn dm_and_invite_take_an_id_or_a_completed_name() {
        let mut app = with_members(chat());
        type_text(&mut app, "/dm @am");
        // Tab completes the name, as in a message.
        app.on_event(key(KeyCode::Tab));
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::StartDm { user_id }] if user_id == "@amy:x"),
            "{commands:?} / {:?}",
            app.composer.text()
        );
        type_text(&mut app, "/invite @new:other.org");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::Invite { user_id, .. }] if user_id == "@new:other.org"),
            "{commands:?}"
        );
        type_text(&mut app, "/room Book club");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::CreateRoom { name }] if name == "Book club"));
    }

    #[test]
    fn a_message_is_pinned_with_p_and_forwarded_with_f() {
        let mut app = chat();
        app.on_event(key(KeyCode::Up));
        let selected = app.selected.clone().unwrap();
        let commands = app.on_event(key(KeyCode::Char('p')));
        assert!(
            matches!(&commands[..], [Command::TogglePin { event_id }] if *event_id == selected)
        );

        app.on_event(key(KeyCode::Char('f')));
        assert!(app.overlays.is_open::<Forward>());
        let names: Vec<&str> = app
            .forward_targets()
            .iter()
            .map(|room| room.name.as_str())
            .collect();
        assert_eq!(names, ["Lunch Club", "Amy Winter", "Dev Chat"]);
        type_text(&mut app, "dev");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::Forward { event_id, room_id }]
                if *event_id == selected && room_id == "!c"),
            "{commands:?}"
        );
        assert!(!app.overlays.is_open::<Forward>());
    }

    #[test]
    fn the_pinned_view_is_for_reading() {
        let mut app = chat();
        type_text(&mut app, "/pins");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::OpenThread { event_id }] if event_id == "pinned"),
            "{commands:?}"
        );
        type_text(&mut app, "hello");
        assert!(app.on_event(key(KeyCode::Enter)).is_empty());
        assert_eq!(app.composer.text(), "hello");
        app.composer.clear();
        let commands = app.on_event(key(KeyCode::Esc));
        assert!(matches!(&commands[..], [Command::CloseThread]));
    }

    #[test]
    fn link_previews_are_asked_for_once_and_only_when_switched_on() {
        let mut app = chat();
        let room_id = app.state.open_room.clone().unwrap();
        let items = vec![message(
            "$l",
            "look: https://example.org/a and https://b.example",
            false,
        )];
        app.on_update(Update::Timeline {
            room_id: room_id.clone(),
            items: items.clone(),
        });
        assert!(app.take_commands().is_empty());

        app.state.prefs.link_previews = true;
        app.on_update(Update::Timeline {
            room_id: room_id.clone(),
            items: items.clone(),
        });
        let asked = app.take_commands();
        assert!(
            matches!(&asked[..], [Command::PreviewLink(url)] if url == "https://example.org/a"),
            "{asked:?}"
        );
        app.on_update(Update::Timeline { room_id, items });
        assert!(app.take_commands().is_empty());
    }

    #[test]
    fn a_key_bound_in_the_config_does_what_the_built_in_one_does() {
        let mut app = chat();
        let bound = [("search".to_owned(), "ctrl+s".to_owned())]
            .into_iter()
            .collect();
        app.keymap = crate::keys::keymap(&bound).0;
        app.on_event(ctrl('s'));
        assert!(app.overlays.is_open::<Search>());
    }

    #[test]
    fn the_recovery_key_is_shown_until_closed_and_can_be_copied() {
        let mut app = chat();
        type_text(&mut app, "/backup");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::Backup { renew: false }]));
        app.on_update(Update::RecoveryKey("EsTc 1234 abcd".into()));
        app.on_event(key(KeyCode::Char('c')));
        assert_eq!(app.take_clipboard().as_deref(), Some("EsTc 1234 abcd"));
        assert!(app.overlays.is_open::<RecoveryKey>());
        app.on_event(key(KeyCode::Esc));
        assert!(!app.overlays.is_open::<RecoveryKey>());
    }

    #[test]
    fn resetting_the_identity_asks_for_the_password_first() {
        let mut app = chat();
        type_text(&mut app, "/reset-identity");
        assert!(app.on_event(key(KeyCode::Enter)).is_empty());
        assert!(app.overlays.is_open::<ResetPrompt>());
        type_text(&mut app, "hunter2");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::ResetIdentity { password }] if password == "hunter2"),
            "{commands:?}"
        );
        // Esc backs out without doing anything.
        type_text(&mut app, "/reset-identity");
        app.on_event(key(KeyCode::Enter));
        assert!(app.on_event(key(KeyCode::Esc)).is_empty());
        assert!(!app.overlays.is_open::<ResetPrompt>());
    }

    #[test]
    fn rooms_and_people_can_share_one_section() {
        let mut app = app();
        app.focus = Focus::Sidebar;
        app.on_event(key(KeyCode::Char('m')));
        assert!(app.state.prefs.mixed_list && app.state.config_dirty);
        assert_eq!(
            app.sidebar_rows(),
            [
                SidebarRow::Header {
                    title: "CHATS".into()
                },
                SidebarRow::Room { index: 0 },
                SidebarRow::Room { index: 1 },
                SidebarRow::Room { index: 2 },
            ]
        );
    }

    #[test]
    fn sections_fold_to_a_summary_and_filtering_looks_inside() {
        let mut app = app();
        app.focus = Focus::Sidebar;
        app.state.rooms[2].unread = 3;
        app.cursor_room = Some(app.state.rooms[0].id.clone());
        app.on_event(key(KeyCode::Char('z')));
        assert_eq!(
            app.sidebar_rows(),
            [
                SidebarRow::Folded {
                    title: "ROOMS".into(),
                    rooms: 2,
                    unread: 3,
                    mentions: 0
                },
                SidebarRow::Header {
                    title: "PEOPLE".into()
                },
                SidebarRow::Room { index: 1 },
            ]
        );
        // The cursor left the folded section for the first room listed.
        assert_eq!(app.cursor_room.as_ref(), Some(&app.state.rooms[1].id));
        // Moving skips the folded rooms.
        app.on_event(key(KeyCode::Up));
        assert_eq!(app.cursor_room.as_ref(), Some(&app.state.rooms[1].id));

        // A filter finds rooms in folded sections too.
        let name = app.state.rooms[0].name.clone();
        app.filter.set(&name);
        assert!(app.sidebar_rows().contains(&SidebarRow::Room { index: 0 }));
        app.filter.clear();

        click_row(&mut app, "ROOMS (folded)");
        assert_eq!(
            app.sidebar_rows()[0],
            SidebarRow::Header {
                title: "ROOMS".into()
            }
        );
        click_row(&mut app, "PEOPLE");
        app.on_event(key(KeyCode::Char('Z')));
        assert!(app.state.folded.is_empty());
    }

    #[test]
    fn y_copies_only_the_code_and_x_unfolds() {
        let mut app = chat();
        let code = Item {
            key: "c".into(),
            event_id: "$c".into(),
            body: "run this:\n```sh\nls -la\n```\nthanks".into(),
            ..Item::default()
        };
        app.on_update(Update::Timeline {
            room_id: "!a".into(),
            items: vec![code],
        });
        // Folded from two lines on, which the drawing finds out.
        app.state.prefs.long_messages = 2;
        crate::ui::render_to_text(&mut app, 80, 24);
        app.on_event(key(KeyCode::Up));
        app.on_event(key(KeyCode::Char('x')));
        assert!(app.expanded.contains("$c"));
        app.on_event(key(KeyCode::Char('y')));
        assert_eq!(app.take_clipboard().as_deref(), Some("ls -la"));

        app.on_hit(
            HitAction::CopyCode {
                event_id: "$c".into(),
                index: 0,
            },
            &mut Vec::new(),
        );
        assert_eq!(app.take_clipboard().as_deref(), Some("ls -la"));
        app.on_hit(
            HitAction::CopyCode {
                event_id: "$c".into(),
                index: 5,
            },
            &mut Vec::new(),
        );
        assert!(app.take_clipboard().is_none());
    }

    #[test]
    fn the_message_box_takes_several_lines() {
        let mut app = chat();
        app.on_event(Event::Paste("```\nls -la\n```\n".into()));
        assert_eq!(app.composer.text(), "```\nls -la\n```");
        // Up moves inside the text first; only from the first line does it
        // go to the conversation.
        app.on_event(key(KeyCode::Up));
        app.on_event(key(KeyCode::Up));
        assert!(app.selected.is_none());
        assert_eq!(app.composer.cursor(), 3);
        app.on_event(key(KeyCode::Down));
        assert_eq!(app.composer.cursor(), 7);

        app.composer.clear();
        app.on_event(key(KeyCode::Char('a')));
        app.on_event(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)));
        app.on_event(key(KeyCode::Char('b')));
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::Send { text, .. }] if text == "a\nb"));
    }

    #[test]
    fn ctrl_v_asks_the_clipboard_and_a_picture_waits_for_enter() {
        let mut app = chat();
        let commands = app.on_event(Event::Key(KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::CONTROL,
        )));
        assert!(matches!(&commands[..], [Command::ClipboardPaste]));

        app.on_update(Update::PastedText("hello\nthere".into()));
        assert_eq!(app.composer.text(), "hello\nthere");
        // With text in the box the picture is not put on top of it.
        app.on_update(Update::PastedImage("/tmp/p.png".into()));
        assert_eq!(app.composer.text(), "hello\nthere");

        app.composer.clear();
        app.on_update(Update::PastedImage("/tmp/p.png".into()));
        assert_eq!(app.composer.text(), "/file /tmp/p.png");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::SendFile { path, .. }] if path == "/tmp/p.png"));
    }

    #[test]
    fn slash_suggests_commands_and_enter_runs_the_ones_without_arguments() {
        let mut app = chat();
        app.on_event(key(KeyCode::Char('/')));
        let Popup { title, rows, .. } = app.popup();
        assert_eq!(title, " commands · Tab ");
        assert_eq!(rows.len(), COMMANDS.len());

        // "/p" narrows it down; Tab completes and leaves room for the argument.
        app.on_event(key(KeyCode::Char('p')));
        let names: Vec<String> = app.popup().rows.into_iter().map(|row| row.0).collect();
        assert_eq!(names, ["/poll", "/paste", "/pins"]);
        app.on_event(key(KeyCode::Tab));
        assert_eq!(app.composer.text(), "/poll ");
        assert!(
            app.command_matches().is_empty(),
            "no popup once the name is done"
        );

        // Enter on a command without argument runs it at once.
        app.composer.clear();
        for c in "/set".chars() {
            app.on_event(key(KeyCode::Char(c)));
        }
        app.on_event(key(KeyCode::Enter));
        assert!(app.overlays.is_open::<SettingsDialog>());
        assert!(app.composer.is_empty());

        // Enter on one with an argument only completes it.
        app.overlays.close::<SettingsDialog>();
        for c in "/th".chars() {
            app.on_event(key(KeyCode::Char(c)));
        }
        app.on_event(key(KeyCode::Enter));
        assert_eq!(app.composer.text(), "/theme ");
    }

    #[test]
    fn long_ids_are_cut_in_the_middle() {
        assert_eq!(short_id("@amy:example.org", 30), "@amy:example.org");
        let long =
            "@11111111-2222-4333-8444-555555555555:66666666-7777-4888-9999-aaaaaaaaaaaa.iserv.host";
        let short = short_id(long, 30);
        assert_eq!(short.chars().count(), 30);
        assert!(
            short.starts_with("@11111111-2222") && short.ends_with(".iserv.host"),
            "{short}"
        );
    }

    #[test]
    fn sidebar_groups_rooms_before_people() {
        assert_eq!(
            app().sidebar_rows(),
            [
                SidebarRow::Header {
                    title: "ROOMS".into()
                },
                SidebarRow::Room { index: 0 },
                SidebarRow::Room { index: 2 },
                SidebarRow::Header {
                    title: "PEOPLE".into()
                },
                SidebarRow::Room { index: 1 },
            ]
        );
    }

    #[test]
    fn pinned_rooms_and_people_move_to_their_own_section_on_top() {
        let mut app = app();
        app.on_event(key(KeyCode::Down)); // cursor on Dev Chat
        let commands = app.on_event(key(KeyCode::Char('p')));
        assert!(
            matches!(&commands[..], [Command::SetFavourite { room_id, value: true }] if room_id == "!c")
        );
        assert_eq!(
            app.sidebar_rows(),
            [
                SidebarRow::Header {
                    title: "PINNED".into()
                },
                SidebarRow::Room { index: 2 },
                SidebarRow::Header {
                    title: "ROOMS".into()
                },
                SidebarRow::Room { index: 0 },
                SidebarRow::Header {
                    title: "PEOPLE".into()
                },
                SidebarRow::Room { index: 1 },
            ]
        );
        // Pressing it again unpins.
        let commands = app.on_event(key(KeyCode::Char('p')));
        assert!(matches!(
            &commands[..],
            [Command::SetFavourite { value: false, .. }]
        ));
    }

    #[test]
    fn right_click_on_a_room_opens_its_menu() {
        let mut app = app();
        app.hits.push(Hit {
            rect: Rect::new(1, 4, 20, 1),
            action: HitAction::OpenRoom("!b".into()),
        });
        let commands = app.on_event(mouse(MouseEventKind::Down(MouseButton::Right), 5, 4));
        assert!(commands.is_empty());
        assert!(
            app.state.open_room.is_none(),
            "right-click must not open the room"
        );
        assert_eq!(
            app.room_menu_rows()
                .into_iter()
                .map(|(_, label)| label)
                .collect::<Vec<_>>(),
            [
                "Open",
                "Pin to top",
                "Move to section…",
                "Low priority",
                "Notify: default",
                "Room info…",
                "Leave…"
            ]
        );

        // Second row: pin.
        app.on_event(key(KeyCode::Down));
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::SetFavourite { room_id, value: true }] if room_id == "!b")
        );
        assert!(!app.overlays.is_open::<RoomMenu>());

        // "Move to section…" opens the section picker for that room.
        app.on_event(mouse(MouseEventKind::Down(MouseButton::Right), 5, 4));
        app.on_hit(
            HitAction::RoomMenuPick(RoomMenuItem::Section),
            &mut Vec::new(),
        );
        assert_eq!(
            app.overlays
                .get::<SectionPicker>()
                .map(|picker| picker.room_id.as_str()),
            Some("!b")
        );
    }

    #[test]
    fn sort_modes_reorder_within_sections() {
        let mut app = app();
        app.state.rooms[2].unread = 4;
        app.on_event(key(KeyCode::Char('s')));
        assert_eq!(app.state.prefs.sort, SortMode::Name);
        assert_eq!(
            app.sidebar_rows()[1..3],
            [SidebarRow::Room { index: 2 }, SidebarRow::Room { index: 0 }]
        );
        app.on_event(key(KeyCode::Char('s')));
        assert_eq!(app.state.prefs.sort, SortMode::Unread);
        assert_eq!(
            app.sidebar_rows()[1..3],
            [SidebarRow::Room { index: 2 }, SidebarRow::Room { index: 0 }]
        );
        app.state.rooms[2].unread = 0;
        assert_eq!(
            app.sidebar_rows()[1..3],
            [SidebarRow::Room { index: 0 }, SidebarRow::Room { index: 2 }]
        );
        assert!(app.state.config_dirty);
    }

    #[test]
    fn enter_opens_the_cursor_room_and_focuses_the_composer() {
        let mut app = app();
        app.on_event(key(KeyCode::Down));
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::Open(id)] if id == "!c"));
        assert_eq!(app.state.open_room.as_deref(), Some("!c"));
        assert_eq!(app.focus, Focus::Composer);
    }

    #[test]
    fn filter_narrows_the_sidebar_and_enter_opens_the_match() {
        let mut app = app();
        app.on_event(ctrl('k'));
        type_text(&mut app, "amy");
        assert_eq!(
            app.sidebar_rows(),
            [
                SidebarRow::Header {
                    title: "PEOPLE".into()
                },
                SidebarRow::Room { index: 1 }
            ]
        );
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::Open(id)] if id == "!b"));
        assert!(app.filter.is_empty());
    }

    #[test]
    fn typing_and_enter_sends_to_the_open_room() {
        let mut app = chat();
        type_text(&mut app, "hi there");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(
            &commands[..],
            [Command::Send { room_id, text, .. }] if room_id == "!a" && text == "hi there"
        ));
        assert!(app.composer.is_empty());
    }

    #[test]
    fn slash_commands_are_not_sent_as_messages() {
        let mut app = chat();
        type_text(&mut app, "/theme nord");
        assert!(app.on_event(key(KeyCode::Enter)).is_empty());
        assert_eq!(app.theme.name, "nord");
        assert!(app.state.config_dirty);
    }

    #[test]
    fn up_from_the_composer_selects_the_newest_message_and_r_replies() {
        let mut app = chat();
        app.on_event(key(KeyCode::Up));
        assert_eq!(app.selected.as_deref(), Some("$3"));
        assert_eq!(app.focus, Focus::Timeline);
        app.on_event(key(KeyCode::Char('r')));
        assert_eq!(app.focus, Focus::Composer);
        assert!(matches!(
            &app.compose,
            Compose::Reply { event_id, label } if event_id == "$3" && label == "Amy: last"
        ));
        type_text(&mut app, "ok");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(
            &commands[..],
            [Command::Reply { event_id, text, .. }] if event_id == "$3" && text == "ok"
        ));
        assert_eq!(app.compose, Compose::Message);
    }

    #[test]
    fn editing_prefills_the_composer_and_escape_cancels_it() {
        let mut app = chat();
        app.on_event(key(KeyCode::Up));
        app.on_event(key(KeyCode::Up)); // "$2", our own message
        app.on_event(key(KeyCode::Char('e')));
        assert_eq!(app.composer.text(), "mine");
        app.on_event(key(KeyCode::Esc));
        assert!(app.composer.is_empty());
        assert_eq!(app.compose, Compose::Message);

        app.on_event(key(KeyCode::Up));
        app.on_event(key(KeyCode::Up));
        app.on_event(key(KeyCode::Char('e')));
        type_text(&mut app, "!");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(
            &commands[..],
            [Command::Edit { event_id, text, .. }] if event_id == "$2" && text == "mine!"
        ));
    }

    #[test]
    fn other_peoples_messages_cannot_be_edited_or_deleted() {
        let mut app = chat();
        app.on_event(key(KeyCode::Up)); // "$3", not ours
        assert!(app.on_event(key(KeyCode::Char('e'))).is_empty());
        assert_eq!(app.compose, Compose::Message);
        app.on_event(key(KeyCode::Char('d')));
        assert_eq!(app.bar, Bar::Actions);
    }

    #[test]
    fn delete_asks_before_sending() {
        let mut app = chat();
        app.on_event(key(KeyCode::Up));
        app.on_event(key(KeyCode::Up));
        assert!(app.on_event(key(KeyCode::Char('d'))).is_empty());
        assert_eq!(app.bar, Bar::ConfirmDelete);
        // Escape backs out without deleting.
        assert!(app.on_event(key(KeyCode::Esc)).is_empty());
        assert_eq!(app.bar, Bar::Actions);
        app.on_event(key(KeyCode::Char('d')));
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::Delete { event_id }] if event_id == "$2"));
        assert!(app.selected.is_none());
    }

    #[test]
    fn reacting_picks_an_emoji_by_number() {
        let mut app = chat();
        app.on_event(key(KeyCode::Up));
        app.on_event(key(KeyCode::Char('a')));
        assert_eq!(app.bar, Bar::Emoji);
        let commands = app.on_event(key(KeyCode::Char('2')));
        assert!(matches!(
            &commands[..],
            [Command::React { event_id, key }] if event_id == "$3" && key == "❤️"
        ));
    }

    #[test]
    fn poll_command_creates_a_poll() {
        let mut app = chat();
        type_text(&mut app, "/poll Lunch? | Pizza | Sushi --multi");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(
            &commands[..],
            [Command::CreatePoll { question, answers, multiple: true, hidden: false }]
                if question == "Lunch?" && answers == &["Pizza".to_owned(), "Sushi".to_owned()]
        ));
    }

    #[test]
    fn incomplete_poll_command_keeps_the_text_and_explains() {
        let mut app = chat();
        type_text(&mut app, "/poll Lunch? | Pizza");
        assert!(app.on_event(key(KeyCode::Enter)).is_empty());
        assert_eq!(app.composer.text(), "/poll Lunch? | Pizza");
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.starts_with("Usage"))
        );
    }

    #[test]
    fn ending_a_poll_needs_confirmation() {
        let mut app = chat();
        app.on_hit(HitAction::AskEndPoll("$2".into()), &mut Vec::new());
        assert_eq!(app.bar, Bar::ConfirmEndPoll);
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::EndPoll { poll_event_id, .. }] if poll_event_id == "$2")
        );
    }

    #[test]
    fn clicking_a_room_region_opens_it() {
        let mut app = app();
        app.hits.push(Hit {
            rect: Rect::new(1, 4, 20, 1),
            action: HitAction::OpenRoom("!b".into()),
        });
        let commands = app.on_event(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
        assert!(matches!(&commands[..], [Command::Open(id)] if id == "!b"));
    }

    #[test]
    fn wheel_scrolls_the_timeline_and_asks_for_older_messages_at_the_top() {
        let mut app = app();
        app.state.open_room = Some("!a".into());
        app.timeline_area = Rect::new(30, 0, 50, 20);
        app.viewport.laid_out(24, 20, "$1", None);
        assert!(
            app.on_event(mouse(MouseEventKind::ScrollUp, 40, 5))
                .is_empty()
        );
        assert_eq!(app.viewport.scroll(), 3);
        let commands = app.on_event(mouse(MouseEventKind::ScrollUp, 40, 5));
        assert_eq!(app.viewport.scroll(), 4);
        assert!(matches!(&commands[..], [Command::Paginate]));
        app.on_event(mouse(MouseEventKind::ScrollDown, 40, 5));
        assert_eq!(app.viewport.scroll(), 1);
    }

    #[test]
    fn new_unread_messages_are_reported_once_startup_has_settled() {
        let mut app = app();
        let mut rooms = app.state.rooms.clone();
        rooms[2].unread = 2;
        // Still warming up: counters filling in are not news.
        app.on_update(Update::Rooms(rooms.clone()));
        assert!(app.take_notifications().is_empty());

        app.state.started = std::time::Instant::now() - std::time::Duration::from_secs(60);
        rooms[2].unread = 3;
        rooms[1].unread = 1;
        app.on_update(Update::Rooms(rooms.clone()));
        assert_eq!(app.take_notifications(), ["Amy Winter", "Dev Chat"]);
        // Same counts again: nothing new.
        app.on_update(Update::Rooms(rooms.clone()));
        assert!(app.take_notifications().is_empty());

        // The room on screen in a focused terminal does not notify.
        app.state.open_room = Some("!c".into());
        rooms[2].unread = 4;
        app.on_update(Update::Rooms(rooms.clone()));
        assert!(app.take_notifications().is_empty());
        // ...but it does once the terminal lost focus.
        app.on_event(Event::FocusLost);
        rooms[2].unread = 5;
        app.on_update(Update::Rooms(rooms));
        assert_eq!(app.take_notifications(), ["Dev Chat"]);
    }

    fn poll(max: u64, voted: &[&str]) -> Poll {
        Poll {
            max_selections: max,
            answers: ["a", "b", "c"]
                .iter()
                .map(|id| PollAnswer {
                    id: (*id).into(),
                    self_voted: voted.contains(id),
                    ..PollAnswer::default()
                })
                .collect(),
            ..Poll::default()
        }
    }

    #[test]
    fn single_choice_click_moves_the_vote() {
        assert_eq!(
            next_selection(&poll(1, &["a"]), "b"),
            Some(vec!["b".to_owned()])
        );
        assert_eq!(next_selection(&poll(1, &["a"]), "a"), None);
    }

    #[test]
    fn multiple_choice_click_toggles_within_the_limit() {
        assert_eq!(
            next_selection(&poll(2, &["a"]), "b"),
            Some(vec!["a".to_owned(), "b".to_owned()])
        );
        assert_eq!(next_selection(&poll(2, &["a", "b"]), "c"), None);
        assert_eq!(
            next_selection(&poll(2, &["a", "b"]), "a"),
            Some(vec!["b".to_owned()])
        );
    }

    #[test]
    fn ended_polls_ignore_clicks() {
        let mut ended = poll(1, &[]);
        ended.is_ended = true;
        assert_eq!(next_selection(&ended, "a"), None);
    }

    fn with_members(mut app: App) -> App {
        app.on_update(Update::Members {
            room_id: "!a".into(),
            members: vec![
                Member {
                    user_id: "@amy:x".into(),
                    name: "Amy Winter".into(),
                },
                Member {
                    user_id: "@ben:x".into(),
                    name: "Ben Okafor".into(),
                },
                Member {
                    user_id: "@amanda:x".into(),
                    name: "Zed (amanda)".into(),
                },
                Member {
                    user_id: "@me:x".into(),
                    name: "Me".into(),
                },
            ],
        });
        app
    }

    #[test]
    fn typing_at_sign_suggests_members_and_tab_completes() {
        let mut app = with_members(chat());
        type_text(&mut app, "hi @am");
        let names: Vec<String> = app.mention_matches().into_iter().map(|m| m.name).collect();
        // Display name and user id both match; never ourselves.
        assert_eq!(names, ["Amy Winter", "Zed (amanda)"]);

        app.on_event(key(KeyCode::Tab));
        assert_eq!(app.composer.text(), "hi @Amy Winter ");
        assert_eq!(
            app.focus,
            Focus::Composer,
            "Tab must complete, not switch panes"
        );
        assert!(app.mention_matches().is_empty());

        type_text(&mut app, "lunch?");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(
            &commands[..],
            [Command::Send { text, mentions, .. }]
                if text == "hi @Amy Winter lunch?" && mentions.len() == 1 && mentions[0].user_id == "@amy:x"
        ));
    }

    #[test]
    fn mention_popup_can_be_navigated_and_dismissed() {
        let mut app = with_members(chat());
        type_text(&mut app, "@");
        assert_eq!(app.mention_matches().len(), 3);
        app.on_event(key(KeyCode::Down));
        app.on_event(key(KeyCode::Enter));
        assert_eq!(app.composer.text(), "@Ben Okafor ");

        type_text(&mut app, "@z");
        app.on_event(key(KeyCode::Esc));
        assert!(app.mention_matches().is_empty());
        assert_eq!(app.focus, Focus::Composer, "Esc only closes the popup");
        // Enter now sends instead of completing.
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::Send { mentions, .. }] if mentions.len() == 1));
    }

    #[test]
    fn email_addresses_do_not_open_the_mention_popup() {
        let mut app = with_members(chat());
        type_text(&mut app, "mail me at amy@am");
        assert!(app.mention_matches().is_empty());
    }

    #[test]
    fn a_mention_removed_while_editing_is_dropped() {
        let mut app = with_members(chat());
        type_text(&mut app, "@be");
        app.on_event(key(KeyCode::Tab));
        app.on_event(ctrl('u'));
        type_text(&mut app, "plain");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::Send { mentions, .. }] if mentions.is_empty()));
    }

    #[test]
    fn copy_puts_the_message_text_on_the_clipboard() {
        let mut app = chat();
        app.on_event(key(KeyCode::Up));
        assert!(app.on_event(key(KeyCode::Char('c'))).is_empty());
        assert_eq!(app.take_clipboard().as_deref(), Some("last"));
        assert!(app.take_clipboard().is_none());
        assert!(app.selected.is_none());

        let poll_item = Item {
            kind: ItemKind::Poll(Poll {
                question: "Lunch?".into(),
                answers: vec![
                    PollAnswer {
                        text: "Pizza".into(),
                        ..PollAnswer::default()
                    },
                    PollAnswer {
                        text: "Sushi".into(),
                        ..PollAnswer::default()
                    },
                ],
                ..Poll::default()
            }),
            ..Item::default()
        };
        assert_eq!(copy_text(&poll_item), "Lunch?\n- Pizza\n- Sushi");
    }

    #[test]
    fn votes_and_ending_use_the_format_of_the_poll() {
        let mut app = chat();
        let mut standard = poll(1, &[]);
        standard.format = PollFormat::Mixed;
        app.state.items[0].kind = ItemKind::Poll(standard);
        let mut commands = Vec::new();
        app.on_hit(
            HitAction::Vote {
                poll_event_id: "$1".into(),
                answer_id: "b".into(),
            },
            &mut commands,
        );
        assert!(matches!(
            &commands[..],
            [Command::Vote {
                format: PollFormat::Mixed,
                ..
            }]
        ));

        app.on_hit(HitAction::AskEndPoll("$1".into()), &mut Vec::new());
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(
            &commands[..],
            [Command::EndPoll {
                format: PollFormat::Mixed,
                ..
            }]
        ));
    }

    #[test]
    fn file_paths_are_suggested_and_completed() {
        let dir = std::env::temp_dir().join(format!("mothrix-paths-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Pictures")).expect("temp dir");
        std::fs::write(dir.join("plan.txt"), "x").expect("file");
        std::fs::write(dir.join("photo.png"), "x").expect("file");
        std::fs::write(dir.join(".hidden"), "x").expect("file");
        let base = dir.to_string_lossy().into_owned();

        let mut app = chat();
        type_text(&mut app, &format!("/file {base}/"));
        let names: Vec<String> = app
            .path_matches()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        // Folders first, hidden files left out.
        assert_eq!(names, ["Pictures/", "photo.png", "plan.txt"]);
        assert_eq!(app.popup().title, " files · Tab ");

        type_text(&mut app, "pl");
        app.on_event(key(KeyCode::Tab));
        assert_eq!(app.composer.text(), format!("/file {base}/plan.txt"));
        // The path is complete: no popup, and Enter sends.
        assert!(app.path_matches().is_empty());
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::SendFile { path, .. }] if *path == format!("{base}/plan.txt"))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_button_starts_in_the_home_folder_and_paste_sends_the_clipboard() {
        let mut app = chat();
        app.on_hit(HitAction::FileTemplate, &mut Vec::new());
        assert_eq!(app.composer.text(), "/file ~/");
        assert_eq!(app.focus, Focus::Composer);

        app.composer.clear();
        type_text(&mut app, "/paste");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::PasteImage]));
    }

    #[test]
    fn a_file_can_be_sent_with_a_caption() {
        let mut app = chat();
        type_text(&mut app, "/file ~/cat.png | our new cat");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::SendFile { path, caption: Some(caption) }]
                if path == "~/cat.png" && caption == "our new cat"),
            "{commands:?}"
        );
    }

    #[test]
    fn file_and_verify_commands() {
        let mut app = chat();
        type_text(&mut app, "/file ~/notes.txt");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::SendFile { path, .. }] if path == "~/notes.txt"));
        type_text(&mut app, "/verify");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::VerifyStart]));
    }

    #[test]
    fn dropping_a_file_prepares_sending_it() {
        let dir = std::env::temp_dir().join(format!("mothrix-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("my report.txt");
        std::fs::write(&file, "x").expect("temp file");
        let path = file.to_string_lossy().into_owned();

        assert_eq!(dropped_file(&format!("'{path}'")), Some(path.clone()));
        assert_eq!(dropped_file(&path.replace(' ', "\\ ")), Some(path.clone()));
        assert_eq!(
            dropped_file(&format!("file://{}", path.replace(' ', "%20"))),
            Some(path.clone())
        );
        assert_eq!(dropped_file("just some pasted words"), None);
        assert_eq!(dropped_file("/definitely/not/there.txt"), None);

        let mut app = chat();
        app.on_event(Event::Paste(format!("'{path}'")));
        assert_eq!(app.composer.text(), format!("/file {path}"));
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::SendFile { path: sent, .. }] if *sent == path));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verification_dialog_takes_the_keys() {
        let mut app = chat();
        app.on_update(Update::Verify(Some(VerifyState::Incoming {
            device: "Phone".into(),
        })));
        // Typing goes nowhere while the dialog is open.
        type_text(&mut app, "x");
        assert!(app.composer.is_empty());
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(
            &commands[..],
            [Command::Verify(VerifyInput::Accept)]
        ));

        app.on_update(Update::Verify(Some(VerifyState::Emojis(vec![(
            "🐶".into(),
            "Dog".into(),
        )]))));
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(
            &commands[..],
            [Command::Verify(VerifyInput::Confirm)]
        ));
        app.on_update(Update::Verify(Some(VerifyState::Emojis(vec![(
            "🐶".into(),
            "Dog".into(),
        )]))));
        let commands = app.on_event(key(KeyCode::Esc));
        assert!(matches!(
            &commands[..],
            [Command::Verify(VerifyInput::Cancel)]
        ));

        // A finished dialog closes on Enter.
        app.on_update(Update::Verify(Some(VerifyState::Done)));
        app.on_event(key(KeyCode::Enter));
        assert!(!app.overlays.is_open::<VerifyState>());
    }

    #[test]
    fn poll_arguments_are_parsed_with_flags_anywhere() {
        assert_eq!(
            parse_poll("--hidden Where to? | Here | There  --multi"),
            Ok((
                "Where to?".to_owned(),
                vec!["Here".to_owned(), "There".to_owned()],
                true,
                true
            ))
        );
        assert!(parse_poll("Only a question").is_err());
        assert!(parse_poll("").is_err());
    }
}
