// SPDX-License-Identifier: MPL-2.0

//! What the app shows, kept on the Rust side: the accounts, the room
//! list, the open room or thread, the dialogs' state. [`View`] is what
//! [`Accounts`] serve (their [`Frontend`]).
//!
//! The app asks for things with an [`Action`] and gets [`Command`]s
//! back for the accounts; what changed leaves as [`Event`]s for the app;
//! what has to be done outside (saving the config, signing in, desktop
//! notifications) leaves as [`Effect`]s. Nothing here knows Swift or UniFFI, so it
//! is tested on plain data.
//!
//! [`Accounts`]: mothrix_core::accounts::Accounts

use std::collections::HashMap;

use base64::Engine;
use mothrix_core::{
    accounts::{self, Account, Frontend},
    client_state::{ClientState, Opening, Taken},
    config::{Config, Shown},
    emoji,
    matrix::{Command, PINNED_VIEW, Update},
    message::{self, Offered},
    model::{Item, ItemKind, Member, Notify, RoomDetails, RoomEntry, SearchHit, VerifyState},
    prefs::Prefs,
    sections::OwnSections,
    sidebar::{Folds, Line, Sidebar},
    theme::{self, Palette},
    verify::VerifyInput,
};
use serde::{Deserialize, Serialize};

use crate::rich;

/// News for the app, as the JSON it receives (`{"type": "sidebar", …}`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Accounts {
        accounts: Vec<Account>,
    },
    /// Every room, for what the app looks up by key (menus, the header).
    Rooms {
        rooms: Vec<RoomEntry>,
    },
    /// The room list as it is drawn.
    Sidebar {
        lines: Vec<Line>,
    },
    /// The room shown (by key), and the thread in it (its first
    /// message's id, or "pinned" for the pinned messages).
    Opened {
        room: Option<String>,
        thread: Option<String>,
    },
    /// The conversation shown, oldest first.
    Timeline {
        room: String,
        thread: Option<String>,
        items: Vec<Message>,
    },
    /// A picture of a message, as a `data:` address.
    Picture {
        event_id: String,
        url: String,
    },
    LinkPreview {
        url: String,
        title: String,
        description: String,
    },
    /// Who is writing in the open room right now.
    Typing {
        room: String,
        names: Vec<String>,
    },
    /// Something to tell the user, like an error of the server.
    Notice {
        text: String,
    },
    Settings(Settings),
    /// A verification going on; `None` closes its dialog.
    Verify {
        state: Option<VerifyState>,
    },
    /// A new recovery key, to show once.
    RecoveryKey {
        key: String,
    },
    RoomDetails {
        details: RoomDetails,
    },
    SignIn(SignIn),
    /// Who can be @-mentioned in the open room.
    Members {
        room: String,
        members: Vec<Member>,
    },
    SearchResults {
        query: String,
        hits: Vec<SearchHit>,
    },
    /// The message to show is loaded now: scroll to it.
    Reveal {
        event_id: String,
    },
    /// An attachment is on disk, for the app to show.
    Media {
        event_id: String,
        path: String,
    },
    /// An attachment is on disk and of a kind that is not opened: for
    /// the app to offer for sharing, under the name its sender gave it.
    MediaKept {
        event_id: String,
        path: String,
        name: String,
    },
    /// A message was not sent, since its conversation (`room`, `thread`)
    /// was not the open one any more: its text, to give back.
    NotSent {
        room: String,
        thread: Option<String>,
        text: String,
    },
    /// The emoji to offer first, the user's favourites.
    Favourites {
        emoji: Vec<String>,
    },
    /// The accounts that did not start, after one of them was removed.
    Problems {
        problems: Vec<Problem>,
    },
}

/// An account that is on the phone but did not start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub profile: String,
    /// The line to show: the profile and why it did not start.
    pub text: String,
}

/// What the app is told when a room has something new (it decides: in
/// the foreground the room list shows it, in the background the push
/// does). Not an [`Event`]: it leaves as an [`Effect`], in its order.
pub fn notify_event(room: &str, title: &str) -> serde_json::Value {
    serde_json::json!({ "type": "notify", "room": room, "title": title })
}

/// A conversation item as the app gets it: with its text's Markdown
/// parsed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Message {
    #[serde(flatten)]
    pub item: Item,
    /// The body as blocks of styled text and code; empty for items whose
    /// body is not a message's text.
    pub blocks: Vec<rich::Block>,
    /// The colour of the sender's name, as the terminal gives it.
    pub sender_color: String,
    /// What can be done to it, and what "copy" takes.
    pub can: Offered,
    pub copy: String,
}

impl Message {
    /// `in_thread`: they are a thread's (or the pinned ones), not the
    /// room's conversation.
    fn of(items: &[Item], in_thread: bool, theme: &Palette) -> Vec<Self> {
        items
            .iter()
            .map(|item| Self {
                can: message::offered(item, in_thread),
                copy: message::copy_text(item),
                sender_color: theme.sender_color(&item.sender_id).to_string(),
                blocks: match item.kind {
                    ItemKind::Text | ItemKind::Notice | ItemKind::Emote => rich::blocks(&item.body),
                    _ => Vec::new(),
                },
                item: item.clone(),
            })
            .collect()
    }
}

/// What the settings dialog and the room list's switches show.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Settings {
    pub prefs: Prefs,
    /// The names of the user's own sections.
    pub sections: Vec<String>,
    pub account_filter: Option<usize>,
    pub unread_only: bool,
    /// The colours in use, and the names of all themes to pick from.
    pub theme: Palette,
    pub themes: Vec<String>,
}

/// How a sign-in from the app goes.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SignIn {
    pub busy: bool,
    /// What the server or the browser sign-in said so far.
    pub lines: Vec<String>,
    pub error: Option<String>,
}

/// Everything the app needs when it (re)loads.
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub accounts: Vec<Account>,
    pub rooms: Vec<RoomEntry>,
    pub lines: Vec<Line>,
    pub open_room: Option<String>,
    pub thread: Option<String>,
    pub items: Vec<Message>,
    pub pictures: HashMap<String, String>,
    pub link_previews: HashMap<String, (String, String)>,
    pub settings: Settings,
    pub verify: Option<VerifyState>,
    pub sign_in: SignIn,
    /// Accounts that could not be started, and why.
    pub problems: Vec<Problem>,
    pub members: Vec<Member>,
    pub favourites: Vec<String>,
    /// Every emoji, by group, for the picker.
    pub emoji: Vec<emoji::Group>,
}

/// What the app asks for.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    // The conversation.
    Open {
        room: String,
    },
    /// "Join" in the app's own question about an invitation: opens (and
    /// so joins) it without asking again.
    AcceptInvitation {
        room: String,
    },
    /// `mentions`: the people picked with @ (their names are in the text).
    Send {
        text: String,
        #[serde(default)]
        mentions: Vec<Member>,
    },
    Reply {
        event_id: String,
        text: String,
        #[serde(default)]
        mentions: Vec<Member>,
    },
    Edit {
        event_id: String,
        text: String,
        #[serde(default)]
        mentions: Vec<Member>,
    },
    Delete {
        event_id: String,
    },
    React {
        event_id: String,
        key: String,
    },
    /// `answer` is the one tapped: picked, or no longer.
    Vote {
        event_id: String,
        answer: String,
    },
    EndPoll {
        event_id: String,
    },
    TogglePin {
        event_id: String,
    },
    OpenThread {
        event_id: String,
    },
    OpenPinned,
    CloseThread,
    /// Back to the room list: the room is no longer in view.
    Close,
    LoadOlder,
    OpenMedia {
        event_id: String,
    },
    /// Send this message of the open room on to another room (of the
    /// same account).
    Forward {
        event_id: String,
        room: String,
    },
    PreviewLink {
        url: String,
    },
    Typing {
        active: bool,
    },
    /// A recorded voice message (a file the app wrote): its length and
    /// loudness over time, 0 to 1.
    SendVoice {
        path: String,
        duration_ms: u64,
        waveform: Vec<f32>,
    },
    /// A file the app read (picked or pasted), in base64.
    Upload {
        name: String,
        data: String,
        caption: Option<String>,
    },

    // The room list.
    Fold {
        key: String,
    },
    UnreadOnly {
        value: bool,
    },
    ShowAccount {
        account: Option<usize>,
    },
    Favourite {
        room: String,
        value: bool,
    },
    LowPriority {
        room: String,
        value: bool,
    },
    Notify {
        room: String,
        mode: Notify,
    },
    /// Into one of the user's own sections (made if new), or out of them.
    MoveToSection {
        room: String,
        section: Option<String>,
    },
    /// One of the user's own sections, with its subsections.
    RenameSection {
        from: String,
        to: String,
    },
    RemoveSection {
        name: String,
    },
    MoveSection {
        name: String,
        down: bool,
    },
    Leave {
        room: String,
    },
    /// Read up to the newest message of a room, without opening it.
    MarkRead {
        room: String,
    },

    // Rooms and people.
    RoomDetails {
        room: String,
    },
    Invite {
        room: String,
        user_id: String,
    },
    Kick {
        room: String,
        user_id: String,
    },
    RenameRoom {
        room: String,
        name: String,
    },
    SetTopic {
        room: String,
        topic: String,
    },
    StartDm {
        account: usize,
        user_id: String,
    },
    CreateRoom {
        account: usize,
        name: String,
    },
    Join {
        account: usize,
        target: String,
    },

    // Encryption.
    VerifyDevice {
        account: usize,
    },
    VerifyUser {
        user_id: String,
    },
    VerifyAnswer {
        answer: Answer,
    },
    /// The verification dialog was closed.
    VerifyClose,
    Recover {
        account: usize,
        key: String,
    },
    Backup {
        account: usize,
        renew: bool,
    },

    // Accounts and settings.
    SignIn {
        server: String,
        user: String,
        password: String,
        name: String,
    },
    /// The phone's push token: have this account's homeserver push to it
    /// through `gateway`, as `app_id` (the gateway's name for the app).
    RegisterPush {
        account: usize,
        token: String,
        app_id: String,
        gateway: String,
        device: String,
        lang: String,
    },
    SignOut {
        account: usize,
    },
    /// Take an account that did not start (one of the snapshot's
    /// `problems`) off this phone.
    RemoveAccount {
        profile: String,
    },
    Prefs {
        prefs: Prefs,
    },

    // Searching, polls, help with writing.
    /// In the open room, or `everywhere`.
    Search {
        text: String,
        everywhere: bool,
    },
    /// Show a message found: its room, scrolled to it.
    ShowHit {
        room: String,
        event_id: String,
    },
    /// Bring a message of the open room into the conversation (paging back
    /// if needed), to scroll to it: `reveal` comes when it is there.
    Reveal {
        event_id: String,
    },
    CreatePoll {
        question: String,
        answers: Vec<String>,
        multiple: bool,
        hidden: bool,
    },
    /// An emoji was put into a message (reactions count by themselves).
    UsedEmoji {
        emoji: String,
    },
    Theme {
        name: String,
    },
}

/// The buttons of the verification dialog.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Accept,
    Confirm,
    Cancel,
}

/// One thing for the world outside to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Write what is shown into the config file (see [`View::shown`]).
    SaveConfig,
    /// Tell the desktop about new messages in a room (its key).
    Notify {
        room: String,
        title: String,
    },
    SignIn(accounts::SignIn),
    SignOut(usize),
    /// Remove the account of this profile, which is on disk but does not
    /// run; [`View::removed`] hears how it went.
    Remove(String),
    /// Keep this file (base64) on disk and send it to the open room.
    Upload {
        /// The conversation it goes into.
        room_id: String,
        thread: Option<String>,
        name: String,
        data: String,
        caption: Option<String>,
    },
    /// Count an emoji in the file of favourites.
    UsedEmoji(String),
}

/// What the app was last told, to tell it only what changed.
#[derive(Default)]
struct Told {
    accounts: Vec<Account>,
    lines: Vec<Line>,
    opened: (Option<String>, Option<String>),
    settings: Option<Settings>,
}

pub struct View {
    /// What core keeps of the accounts and rooms.
    state: ClientState,
    pictures: HashMap<String, String>,
    verify: Option<VerifyState>,
    sign_in: SignIn,
    problems: Vec<Problem>,
    told: Told,
    events: Vec<Event>,
    effects: Vec<Effect>,
    /// Commands that came up outside an action (a room to show once it
    /// is listed).
    later: Vec<Command>,
    emoji_usage: emoji::Usage,
    theme: Palette,
    /// Built-in and own themes.
    themes: Vec<Palette>,
    /// The spelling languages, for tidying what is typed for LanguageTool.
    dictionaries: Vec<String>,
}

impl View {
    /// For these accounts (in the config's order), with the config file's
    /// names, sections and settings.
    pub fn new(mut accounts: Vec<Account>, problems: Vec<Problem>, config: &Config) -> Self {
        config.name_accounts(&mut accounts);
        let account_filter = config.account_filter(&accounts);
        let mut state = ClientState::new(accounts);
        state.account_filter = account_filter;
        state.prefs = config.prefs.clone();
        state.sections = OwnSections::new(config.sections.clone());
        state.folded = Folds::new(config.folded.iter().cloned());
        let mut view = Self {
            state,
            theme: config.palette(),
            themes: config.all_themes().0,
            dictionaries: config.dictionaries(),
            pictures: HashMap::new(),
            verify: None,
            sign_in: SignIn::default(),
            problems,
            told: Told::default(),
            events: Vec::new(),
            effects: Vec::new(),
            later: Vec::new(),
            emoji_usage: emoji::Usage::default(),
        };
        // The app gets these with the snapshot.
        view.told.accounts = view.state.accounts.clone();
        view.told.lines = view.lines();
        view.told.settings = Some(view.settings());
        view
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            accounts: self.state.accounts.clone(),
            rooms: self.state.rooms.clone(),
            lines: self.lines(),
            open_room: self.state.open_room.clone(),
            thread: self.state.open_thread.clone(),
            items: Message::of(
                &self.state.items,
                self.state.open_thread.is_some(),
                &self.theme,
            ),
            pictures: self.pictures.clone(),
            link_previews: self.state.link_previews.clone(),
            settings: self.settings(),
            verify: self.verify.clone(),
            sign_in: self.sign_in.clone(),
            problems: self.problems.clone(),
            members: self.state.members.clone(),
            favourites: self.favourites(),
            emoji: emoji::groups(),
        }
    }

    /// What the config file keeps of what is shown.
    pub fn shown(&self) -> Shown<'_> {
        self.state.shown()
    }

    fn sidebar(&self) -> Sidebar<'_> {
        Sidebar {
            rooms: &self.state.rooms,
            accounts: &self.state.accounts,
            sections: &self.state.sections,
            prefs: &self.state.prefs,
            folded: &self.state.folded,
            filter: "",
            unread_only: self.state.unread_only,
            account_filter: self.state.account_filter,
            open_room: self.state.open_room.as_deref(),
        }
    }

    fn lines(&self) -> Vec<Line> {
        self.sidebar().lines()
    }

    fn settings(&self) -> Settings {
        Settings {
            prefs: self.state.prefs.clone(),
            sections: self
                .state
                .sections
                .iter()
                .map(|section| section.name.clone())
                .collect(),
            account_filter: self.state.account_filter,
            unread_only: self.state.unread_only,
            theme: self.theme.clone(),
            themes: self
                .themes
                .iter()
                .map(|theme| theme.name.to_string())
                .collect(),
        }
    }

    /// The name of the theme in use, for the config file.
    pub fn theme_name(&self) -> &str {
        &self.theme.name
    }

    fn open_entry(&self) -> Option<&RoomEntry> {
        let open = self.state.open_room.as_ref()?;
        self.state.rooms.iter().find(|room| room.id == *open)
    }

    /// What changed since the last call, in order.
    pub fn take_events(&mut self) -> Vec<Event> {
        if self.state.accounts != self.told.accounts {
            self.told.accounts = self.state.accounts.clone();
            self.events.push(Event::Accounts {
                accounts: self.state.accounts.clone(),
            });
        }
        let lines = self.lines();
        if lines != self.told.lines {
            self.told.lines = lines.clone();
            self.events.push(Event::Sidebar { lines });
        }
        let opened = (self.state.open_room.clone(), self.state.open_thread.clone());
        if opened != self.told.opened {
            self.told.opened = opened.clone();
            self.events.push(Event::Opened {
                room: opened.0,
                thread: opened.1,
            });
        }
        let settings = self.settings();
        if self.told.settings.as_ref() != Some(&settings) {
            self.told.settings = Some(settings.clone());
            self.events.push(Event::Settings(settings));
        }
        std::mem::take(&mut self.events)
    }

    /// Commands for the accounts that came up outside an action.
    pub fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.later)
    }

    /// What is to be done outside since the last call, in order.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        let mut effects = std::mem::take(&mut self.effects);
        if std::mem::take(&mut self.state.config_dirty) {
            effects.push(Effect::SaveConfig);
        }
        effects
    }

    /// A line for the user, from outside an action.
    pub fn say(&mut self, text: &str) {
        self.notice(text);
    }

    fn notice(&mut self, text: impl Into<String>) {
        self.events.push(Event::Notice { text: text.into() });
    }

    /// The app got or lost the keyboard.
    pub fn focus(&mut self, focused: bool) {
        self.state.focused = focused;
    }

    /// The emoji the user takes most.
    pub fn set_emoji_usage(&mut self, emoji: emoji::Usage) {
        self.emoji_usage = emoji;
    }

    fn favourites(&self) -> Vec<String> {
        self.emoji_usage.favourites(24)
    }

    fn used_emoji(&mut self, emoji: String) {
        self.emoji_usage.note(&emoji);
        self.effects.push(Effect::UsedEmoji(emoji));
        self.events.push(Event::Favourites {
            emoji: self.favourites(),
        });
    }

    /// A line from a sign-in under way.
    pub fn sign_in_progress(&mut self, line: String) {
        self.sign_in.lines.push(line);
        self.events.push(Event::SignIn(self.sign_in.clone()));
    }

    /// A sign-in came to an end: the new account, or what went wrong.
    pub fn sign_in_done(&mut self, result: Result<Account, String>) {
        match result {
            Ok(account) => {
                self.notice(format!("Signed in as {}.", account.user_id));
                self.state.accounts.push(account);
                self.sign_in = SignIn::default();
                self.state.config_dirty = true;
            }
            Err(error) => {
                self.sign_in.busy = false;
                self.sign_in.error = Some(error);
            }
        }
        self.events.push(Event::SignIn(self.sign_in.clone()));
    }

    /// The account the commands of this action go to, when they name no
    /// room.
    fn for_account(&mut self, account: usize) -> bool {
        if account < self.state.accounts.len() {
            self.state.named_account = Some(account);
            true
        } else {
            false
        }
    }

    /// Do what the app asked for the conversation it showed then (`room`,
    /// and the `thread` in it), if that is still the one shown here: a
    /// poll sheet, a file picker or a recording begun in one chat does
    /// not send into another opened meanwhile (a notification tapped),
    /// nor a message of the room into a thread of it.
    pub fn act_in(&mut self, action: Action, room: &str, thread: Option<&str>) -> Vec<Command> {
        // A link is looked up for the room its message was seen in (the
        // thread makes no difference to whom is asked), or not at all,
        // and without a word: nobody sent anything.
        if matches!(action, Action::PreviewLink { .. }) {
            return match self.state.open_room.as_deref() == Some(room) {
                true => self.act(action),
                false => Vec::new(),
            };
        }
        if self.state.open_room.as_deref() != Some(room)
            || self.state.open_thread.as_deref() != thread
        {
            self.notice("Not sent: another chat was opened meanwhile.");
            // The app emptied its message box when it sent this: the
            // text goes back to it (an answer's too, as a message).
            if let Action::Send { text, .. } | Action::Reply { text, .. } = action {
                self.events.push(Event::NotSent {
                    room: room.to_owned(),
                    thread: thread.map(str::to_owned),
                    text,
                });
            }
            return Vec::new();
        }
        self.act(action)
    }

    /// Do what the app asked. Returns the commands for the accounts.
    pub fn act(&mut self, action: Action) -> Vec<Command> {
        self.state.named_account = None;
        match action {
            Action::Open { room } => return self.open(room),
            Action::AcceptInvitation { room } => {
                let opening = self.state.accept(&room);
                return self.opened(room, opening);
            }
            Action::Send { text, mentions } => {
                let text = text.trim();
                match &self.state.open_room {
                    // Nothing is written among the pinned messages.
                    Some(room) if !text.is_empty() && !self.state.pinned_view() => {
                        return vec![
                            Command::Send {
                                room_id: room.clone(),
                                thread: self.state.open_thread.clone(),
                                text: text.to_owned(),
                                mentions: message::mentions_kept(text, mentions),
                            },
                            Command::Typing(false),
                        ];
                    }
                    _ => {}
                }
            }
            // What is done to a message needs its conversation.
            Action::Reply { .. }
            | Action::Edit { .. }
            | Action::Delete { .. }
            | Action::React { .. }
            | Action::TogglePin { .. }
            | Action::OpenMedia { .. }
                if self.state.open_room.is_none() => {}
            Action::Reply {
                event_id,
                text,
                mentions,
            } if !text.trim().is_empty() => {
                let text = text.trim().to_owned();
                return vec![Command::Reply {
                    event_id,
                    mentions: message::mentions_kept(&text, mentions),
                    text,
                }];
            }
            Action::Edit {
                event_id,
                text,
                mentions,
            } if !text.trim().is_empty() => {
                let text = text.trim().to_owned();
                return vec![Command::Edit {
                    event_id,
                    mentions: message::mentions_kept(&text, mentions),
                    text,
                }];
            }
            Action::Reply { .. } | Action::Edit { .. } => {}
            Action::Delete { event_id } => return vec![Command::Delete { event_id }],
            Action::React { event_id, key } => {
                if message::takes_emoji(&self.state.items, &event_id, &key) {
                    self.used_emoji(key.clone());
                }
                return vec![Command::React { event_id, key }];
            }
            Action::UsedEmoji { emoji } => self.used_emoji(emoji),
            Action::Theme { name } => {
                if let Some(found) = theme::find(&self.themes, &name) {
                    self.theme = found.clone();
                    self.state.config_dirty = true;
                    // Names take the new theme's colours.
                    self.tell_items();
                }
            }
            Action::Search { text, everywhere } => {
                let query = text.trim().to_owned();
                if !query.is_empty() && (everywhere || self.state.open_room.is_some()) {
                    return vec![Command::Search {
                        query,
                        room_id: if everywhere {
                            None
                        } else {
                            self.state.open_room.clone()
                        },
                    }];
                }
            }
            Action::Reveal { event_id } if self.state.open_room.is_some() => {
                return vec![Command::Reveal { event_id }];
            }
            Action::Reveal { .. } => {}
            Action::ShowHit { room, event_id } => {
                // The open room again: out of its thread.
                let mut commands = self.open(room.clone());
                // Not in another room that stays open because the hit's
                // own did not open (no longer listed, or an invitation).
                if self.state.open_room.as_deref() == Some(room.as_str()) {
                    commands.push(Command::Reveal { event_id });
                }
                return commands;
            }
            Action::CreatePoll {
                question,
                answers,
                multiple,
                hidden,
            } => match (
                message::new_poll(&question, &answers),
                self.state.conversation(),
            ) {
                (Ok((question, answers)), Some((room_id, thread))) => {
                    return vec![Command::CreatePoll {
                        room_id,
                        thread,
                        question,
                        answers,
                        multiple,
                        hidden,
                    }];
                }
                (Ok(_), None) => self.notice("Open a room first."),
                (Err(problem), _) => self.notice(problem),
            },
            Action::Vote { event_id, answer } => {
                if let Some(poll) = message::poll_of(&self.state.items, &event_id)
                    && let Some(answers) = message::vote(poll, &answer)
                {
                    return vec![Command::Vote {
                        format: poll.format,
                        poll_event_id: event_id,
                        answers,
                    }];
                }
            }
            Action::EndPoll { event_id } => {
                if let Some(poll) = message::poll_of(&self.state.items, &event_id) {
                    return vec![Command::EndPoll {
                        format: poll.format,
                        poll_event_id: event_id,
                    }];
                }
            }
            Action::TogglePin { event_id } => return vec![Command::TogglePin { event_id }],
            Action::OpenThread { event_id } => return self.open_thread(event_id),
            Action::OpenPinned => return self.open_thread(PINNED_VIEW.to_owned()),
            Action::Close => {
                let account = self.open_entry().map(|entry| entry.account);
                self.close();
                if let Some(account) = account
                    && self.for_account(account)
                {
                    return vec![Command::Close];
                }
            }
            Action::CloseThread => {
                if self.state.leave_thread() {
                    return vec![Command::CloseThread];
                }
            }
            Action::LoadOlder if self.state.open_room.is_some() => return vec![Command::Paginate],
            Action::LoadOlder => {}
            Action::OpenMedia { event_id } => return vec![Command::OpenMedia { event_id }],
            Action::Forward { event_id, room } if self.state.open_room.is_some() => {
                let item = self
                    .state
                    .items
                    .iter()
                    .find(|item| item.event_id == event_id);
                if item.is_some_and(|item| !message::offered(item, false).forward) {
                    self.notice("Only messages and files can be forwarded.");
                    return Vec::new();
                }
                return vec![Command::Forward {
                    event_id,
                    room_id: room,
                }];
            }
            Action::Forward { .. } => {}
            Action::PreviewLink { url } => {
                // Of the open room: its account's homeserver is asked.
                if self.state.prefs.link_previews
                    && let Some(room_id) = self.state.open_room.clone()
                    && self.state.link_previews_asked.insert(url.clone())
                {
                    return vec![Command::PreviewLink { room_id, url }];
                }
            }
            Action::Typing { active } => {
                if self.state.prefs.typing_notices && self.state.open_room.is_some() {
                    return vec![Command::Typing(active)];
                }
            }
            Action::SendVoice {
                path,
                duration_ms,
                waveform,
            } => match self.state.conversation() {
                Some((room_id, thread)) => {
                    return vec![Command::SendVoice {
                        room_id,
                        thread,
                        path,
                        duration_ms,
                        waveform,
                    }];
                }
                None => self.notice("Open a room first."),
            },
            // The file is kept on disk first, outside: it goes into the
            // conversation shown now, whatever is shown by then.
            Action::Upload {
                name,
                data,
                caption,
            } => match self.state.conversation() {
                Some((room_id, thread)) => self.effects.push(Effect::Upload {
                    room_id,
                    thread,
                    name,
                    data,
                    caption,
                }),
                None => self.notice("Open a room first."),
            },

            Action::Fold { key } => {
                self.state.folded.toggle(&key);
                self.state.config_dirty = true;
            }
            Action::UnreadOnly { value } => self.state.unread_only = value,
            Action::ShowAccount { account } => {
                self.state.account_filter =
                    account.filter(|account| *account < self.state.accounts.len());
                self.state.config_dirty = true;
            }
            Action::Favourite { room, value } => {
                return vec![Command::SetFavourite {
                    room_id: room,
                    value,
                }];
            }
            Action::LowPriority { room, value } => {
                return vec![Command::SetLowPriority {
                    room_id: room,
                    value,
                }];
            }
            Action::Notify { room, mode } => {
                return vec![Command::SetNotify {
                    room_id: room,
                    mode,
                }];
            }
            Action::MoveToSection { room, section } => self.move_to_section(room, section),
            Action::RenameSection { from, to } => {
                match self.state.sections.rename(
                    &from,
                    &to,
                    &mut self.state.folded,
                    &self.state.accounts,
                ) {
                    Ok(_) => self.state.config_dirty = true,
                    Err(reason) => self.notice(reason),
                }
            }
            Action::RemoveSection { name } => {
                self.state
                    .sections
                    .remove(&name, &mut self.state.folded, &self.state.accounts);
                self.state.config_dirty = true;
            }
            Action::MoveSection { name, down } => {
                if self.state.sections.move_section(&name, down) {
                    self.state.config_dirty = true;
                }
            }
            Action::MarkRead { room } => return vec![Command::MarkRead { room_id: room }],
            Action::Leave { room } => {
                if self.state.open_room.as_ref() == Some(&room) {
                    self.close();
                }
                return vec![Command::Leave { room_id: room }];
            }

            Action::RoomDetails { room } => return vec![Command::RoomDetails { room_id: room }],
            Action::Invite { room, user_id } => {
                return vec![Command::Invite {
                    room_id: room,
                    user_id: user_id.trim().to_owned(),
                }];
            }
            Action::Kick { room, user_id } => {
                return vec![Command::Kick {
                    room_id: room,
                    user_id,
                }];
            }
            Action::RenameRoom { room, name } => {
                return vec![Command::SetRoomName {
                    room_id: room,
                    name: name.trim().to_owned(),
                }];
            }
            Action::SetTopic { room, topic } => {
                return vec![Command::SetTopic {
                    room_id: room,
                    topic: topic.trim().to_owned(),
                }];
            }
            Action::StartDm { account, user_id } if self.for_account(account) => {
                return vec![Command::StartDm {
                    user_id: user_id.trim().to_owned(),
                }];
            }
            Action::CreateRoom { account, name } if self.for_account(account) => {
                return vec![Command::CreateRoom {
                    name: name.trim().to_owned(),
                }];
            }
            Action::Join { account, target } if self.for_account(account) => {
                return vec![Command::Join {
                    target: target.trim().to_owned(),
                }];
            }
            Action::StartDm { .. } | Action::CreateRoom { .. } | Action::Join { .. } => {}

            Action::VerifyDevice { account } if self.for_account(account) => {
                return vec![Command::VerifyStart];
            }
            Action::VerifyUser { user_id } => return vec![Command::VerifyUser { user_id }],
            Action::VerifyAnswer { answer } => {
                return vec![Command::Verify(match answer {
                    Answer::Accept => VerifyInput::Accept,
                    Answer::Confirm => VerifyInput::Confirm,
                    Answer::Cancel => VerifyInput::Cancel,
                })];
            }
            Action::VerifyClose => {
                let running = !matches!(
                    self.verify.take(),
                    None | Some(VerifyState::Done | VerifyState::Failed(_))
                );
                self.events.push(Event::Verify { state: None });
                if running {
                    return vec![Command::Verify(VerifyInput::Cancel)];
                }
            }
            Action::Recover { account, key } if self.for_account(account) => {
                return vec![Command::Recover(key.trim().to_owned())];
            }
            Action::Backup { account, renew } if self.for_account(account) => {
                return vec![Command::Backup { renew }];
            }
            Action::RegisterPush {
                account,
                token,
                app_id,
                gateway,
                device,
                lang,
            } if self.for_account(account) => {
                return vec![Command::SetPusher(mothrix_core::push::Target {
                    gateway,
                    app_id,
                    pushkey: token,
                    device_name: device,
                    lang,
                    profile: self.state.accounts[account].profile.clone(),
                })];
            }
            Action::VerifyDevice { .. }
            | Action::Recover { .. }
            | Action::Backup { .. }
            | Action::RegisterPush { .. } => {}

            Action::SignIn {
                server,
                user,
                password,
                name,
            } => self.start_sign_in(server, user, password, name),
            Action::SignOut { account } if account < self.state.accounts.len() => {
                self.effects.push(Effect::SignOut(account));
            }
            Action::SignOut { .. } => {}
            // Only one that is listed as not started.
            Action::RemoveAccount { profile } => {
                if self
                    .problems
                    .iter()
                    .any(|problem| problem.profile == profile)
                {
                    self.effects.push(Effect::Remove(profile));
                }
            }
            Action::Prefs { prefs } => {
                let prefs = prefs.tidied(&self.dictionaries);
                if !prefs.link_previews {
                    self.state.link_previews_asked.clear();
                }
                if prefs.images == "off" {
                    self.pictures.clear();
                }
                if !prefs.typing_notices
                    && self.state.prefs.typing_notices
                    && self.state.open_room.is_some()
                {
                    self.state.prefs = prefs;
                    self.state.config_dirty = true;
                    return vec![Command::Typing(false)];
                }
                self.state.prefs = prefs;
                self.state.config_dirty = true;
            }
        }
        Vec::new()
    }

    /// Show a room (by its key).
    fn open(&mut self, room: String) -> Vec<Command> {
        // Again: out of a thread, back to the room.
        let opening = self.state.open(&room);
        self.opened(room, opening)
    }

    /// Show a room nobody tapped just now (one the account says to go
    /// to, one that waited to be listed): an invitation is never joined
    /// this way.
    fn show(&mut self, room: String) -> Vec<Command> {
        let opening = self.state.show(&room);
        self.opened(room, opening)
    }

    /// What follows from opening `room` here.
    fn opened(&mut self, room: String, opening: Opening) -> Vec<Command> {
        // An invitation is not joined by one tap (on the list, or on the
        // notification "X invited you"): the second one joins.
        if let Opening::Invitation { name } = &opening {
            self.notice(format!(
                "{name} is an invitation: open it again to join it."
            ));
        }
        if let Opening::Opened { .. } = opening {
            self.pictures.clear();
            self.events.push(Event::Timeline {
                room,
                thread: None,
                items: Vec::new(),
            });
        }
        opening.commands()
    }

    fn close(&mut self) {
        self.state.close();
        self.pictures.clear();
    }

    fn open_thread(&mut self, root: String) -> Vec<Command> {
        if !self.state.enter_thread(&root) {
            return Vec::new();
        }
        vec![Command::OpenThread { event_id: root }]
    }

    /// The conversation shown, to the app.
    fn tell_items(&mut self) {
        if let Some(room) = self.state.open_room.clone() {
            self.events.push(Event::Timeline {
                room,
                thread: self.state.open_thread.clone(),
                items: Message::of(
                    &self.state.items,
                    self.state.open_thread.is_some(),
                    &self.theme,
                ),
            });
        }
    }

    /// A click on a notification about this room.
    pub fn notification_clicked(&mut self, room: String) -> Vec<Command> {
        // Tapped before the rooms are known (the app was not running):
        // it opens when its account's list has come.
        if !self.state.rooms.iter().any(|entry| entry.id == room) {
            self.state.goto = Some(room);
            return Vec::new();
        }
        self.open(room)
    }

    /// Removing an account that did not start came to an end.
    pub fn removed(&mut self, profile: &str, result: Result<Option<String>, String>) {
        match result {
            Ok(left) => {
                self.problems.retain(|problem| problem.profile != profile);
                self.events.push(Event::Problems {
                    problems: self.problems.clone(),
                });
                // Its name is free again: the next account under it does
                // not take over its rooms in the sections, nor its folds.
                let prefixes = [accounts::prefix_for(profile)];
                let mut changed = self.state.sections.forget_account(&prefixes, 0);
                changed |= self.state.folded.forget_profile(profile);
                self.state.config_dirty |= changed;
                // Still signed in on the server: its pusher goes on
                // sending this phone notifications nobody can read.
                let left = left.map(|left| format!(" {left}")).unwrap_or_default();
                self.notice(format!("'{profile}' is removed from this phone.{left}"));
            }
            Err(error) => self.notice(format!("'{profile}' could not be removed: {error}")),
        }
    }

    fn move_to_section(&mut self, room: String, section: Option<String>) {
        let into_one = section
            .as_deref()
            .is_some_and(|name| !name.trim().is_empty());
        if let Err(reason) = self.state.sections.assign(&room, section.as_deref()) {
            self.notice(reason);
            return;
        }
        if into_one && !self.state.prefs.mixed_list {
            self.state.prefs.mixed_list = true;
            self.notice("Your own sections show in the mixed list; it is on now.");
        }
        self.state.config_dirty = true;
    }

    fn start_sign_in(&mut self, server: String, user: String, password: String, name: String) {
        if self.sign_in.busy {
            return;
        }
        self.sign_in = SignIn::default();
        let taken: Vec<String> = self
            .state
            .accounts
            .iter()
            .map(|account| account.profile.clone())
            .collect();
        match accounts::SignIn::from_form(&server, &user, &password, &name, &taken) {
            Ok(request) => {
                self.sign_in.busy = true;
                self.sign_in.lines.push("Signing in…".to_owned());
                self.effects.push(Effect::SignIn(request));
            }
            Err(problem) => self.sign_in.error = Some(problem.text().to_owned()),
        }
        self.events.push(Event::SignIn(self.sign_in.clone()));
    }
}

impl Frontend for View {
    fn state(&self) -> &ClientState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut ClientState {
        &mut self.state
    }

    fn on_update(&mut self, update: Update) {
        let update = match self.state.take(update) {
            Taken::Rooms { news, show } => {
                for news in news {
                    self.effects.push(Effect::Notify {
                        room: news.room,
                        title: news.title,
                    });
                }
                self.events.push(Event::Rooms {
                    rooms: self.state.rooms.clone(),
                });
                if let Some(room) = show {
                    let commands = self.show(room);
                    self.later.extend(commands);
                }
                return;
            }
            Taken::Show(room) => {
                let commands = self.show(room);
                self.later.extend(commands);
                return;
            }
            // `take_events` says that the thread is left.
            Taken::Items | Taken::LeftThread => return self.tell_items(),
            Taken::Typing => {
                if let Some(room) = self.state.open_room.clone() {
                    self.events.push(Event::Typing {
                        room,
                        names: self.state.typing.clone(),
                    });
                }
                return;
            }
            Taken::Members => {
                if let Some(room) = self.state.open_room.clone() {
                    self.events.push(Event::Members {
                        room,
                        members: self.state.members.clone(),
                    });
                }
                return;
            }
            Taken::LinkPreview(url) => {
                if let Some((title, description)) = self.state.link_previews.get(&url).cloned() {
                    self.events.push(Event::LinkPreview {
                        url,
                        title,
                        description,
                    });
                }
                return;
            }
            Taken::Nothing => return,
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
            Update::Preview { event_id, preview } => {
                if self.state.prefs.images != "off" && !preview.png.is_empty() {
                    let url = format!(
                        "data:image/png;base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(&preview.png)
                    );
                    self.pictures.insert(event_id.clone(), url.clone());
                    self.events.push(Event::Picture { event_id, url });
                }
            }
            Update::Verify(state) => {
                self.verify = state.clone();
                self.events.push(Event::Verify { state });
            }
            Update::RecoveryKey(key) => self.events.push(Event::RecoveryKey { key }),
            Update::RoomDetails(details) => self.events.push(Event::RoomDetails { details }),
            Update::Notice(text) => self.notice(text),
            Update::SearchResults { query, hits } => {
                self.events.push(Event::SearchResults { query, hits });
            }
            Update::Reveal(event_id) => self.events.push(Event::Reveal { event_id }),
            Update::MediaFile { event_id, path } => {
                self.events.push(Event::Media { event_id, path })
            }
            // The app's own folders are none its user gets to: no use
            // in saying where it lies. The app offers to pass it on.
            Update::MediaKept {
                event_id,
                path,
                name,
            } => {
                self.notice(format!(
                    "{name} was saved and not opened: mothrix only opens pictures, sound, \
                     video and documents. It can be shared from its message."
                ));
                self.events.push(Event::MediaKept {
                    event_id,
                    path,
                    name,
                });
            }
            Update::NotSent {
                room_id,
                thread,
                text,
            } => {
                self.notice("Not sent: that conversation is no longer open.");
                self.events.push(Event::NotSent {
                    room: room_id,
                    thread,
                    text,
                });
            }
            // Taken by `Accounts::deliver`: into the sections, on to the
            // account of the room a message is forwarded to.
            Update::Sections(_) | Update::ToForward { .. } => {}
            // Not for the app: what the terminal pastes.
            Update::PastedImage(_) | Update::PastedText(_) => {}
            // `Accounts::deliver` takes these into the accounts.
            Update::OwnName(_) | Update::Sync(_) | Update::Verified(_) => {}
        }
    }

    fn account_gone(&mut self, closed: bool) {
        if closed {
            self.pictures.clear();
        }
        self.events.push(Event::Rooms {
            rooms: self.state.rooms.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use mothrix_core::client_state::NOTIFICATION_WARMUP;
    use mothrix_core::{
        matrix::SyncStatus,
        model::{Poll, PollFormat, Preview},
        sidebar::{Row, Target},
    };

    use super::*;

    fn account(profile: &str) -> Account {
        Account {
            profile: profile.into(),
            label: profile.into(),
            user_id: format!("@me:{profile}"),
            own_name: None,
            sync: SyncStatus::Connecting,
            verified: None,
        }
    }

    fn room(id: &str, account: usize) -> RoomEntry {
        RoomEntry {
            id: id.into(),
            name: id.into(),
            account,
            ..RoomEntry::default()
        }
    }

    fn item(body: &str) -> Item {
        Item {
            body: body.into(),
            ..Item::default()
        }
    }

    fn view_with(config: &Config) -> View {
        let mut view = View::new(
            vec![account("default"), account("work")],
            Vec::new(),
            config,
        );
        view.on_update(Update::Rooms(vec![room("!a", 0), room("work/!b", 1)]));
        view.take_events();
        view.take_effects();
        view
    }

    fn view() -> View {
        view_with(&Config::default())
    }

    fn send(text: &str) -> Action {
        Action::Send {
            text: text.into(),
            mentions: Vec::new(),
        }
    }

    fn act(view: &mut View, json: serde_json::Value) -> Vec<Command> {
        view.act(serde_json::from_value(json).expect("an action the app can send"))
    }

    #[test]
    fn only_the_open_room_reaches_the_app() {
        let mut view = view();
        assert_eq!(
            view.act(Action::Open {
                room: "work/!b".into()
            })
            .len(),
            1
        );
        assert!(
            view.act(Action::Open {
                room: "!gone".into()
            })
            .is_empty()
        );
        view.on_update(Update::Timeline {
            room_id: "!a".into(),
            items: vec![item("elsewhere")],
        });
        view.on_update(Update::Timeline {
            room_id: "work/!b".into(),
            items: vec![item("hi")],
        });
        let timelines: Vec<Event> = view
            .take_events()
            .into_iter()
            .filter(|event| matches!(event, Event::Timeline { .. }))
            .collect();
        assert_eq!(
            timelines,
            [
                Event::Timeline {
                    room: "work/!b".into(),
                    thread: None,
                    items: Vec::new()
                },
                Event::Timeline {
                    room: "work/!b".into(),
                    thread: None,
                    items: Message::of(&[item("hi")], false, &theme::default_palette())
                },
            ]
        );
        assert_eq!(
            view.snapshot().items,
            Message::of(&[item("hi")], false, &theme::default_palette())
        );
        assert_eq!(view.snapshot().items[0].blocks.len(), 1);
        // Commands without a room go to the open room's account.
        assert_eq!(view.active_account(), 1);
    }

    #[test]
    fn the_app_is_told_what_is_open() {
        let mut view = view();
        view.act(Action::Open { room: "!a".into() });
        assert!(view.take_events().contains(&Event::Opened {
            room: Some("!a".into()),
            thread: None
        }));
    }

    #[test]
    fn a_thread_replaces_the_conversation_until_it_is_closed() {
        let mut view = view();
        view.act(Action::Open { room: "!a".into() });
        assert!(matches!(
            &view.act(Action::OpenThread { event_id: "$root".into() })[..],
            [Command::OpenThread { event_id }] if event_id == "$root"
        ));
        // The room's own conversation no longer reaches the app.
        view.on_update(Update::Timeline {
            room_id: "!a".into(),
            items: vec![item("room")],
        });
        view.on_update(Update::Thread {
            room_id: "!a".into(),
            root: "$root".into(),
            items: vec![item("in the thread")],
        });
        assert_eq!(view.snapshot().items[0].item, item("in the thread"));
        assert_eq!(view.snapshot().thread.as_deref(), Some("$root"));
        // Clicking the open room again leads back out.
        assert!(matches!(
            &view.act(Action::Open { room: "!a".into() })[..],
            [Command::CloseThread]
        ));
        assert_eq!(view.snapshot().thread, None);
        assert!(matches!(
            &view.act(Action::OpenPinned)[..],
            [Command::OpenThread { event_id }] if event_id == PINNED_VIEW
        ));
    }

    #[test]
    fn a_thread_that_cannot_be_opened_leads_back_to_the_room() {
        let mut view = view();
        view.act(Action::Open { room: "!a".into() });
        view.act(Action::OpenThread {
            event_id: "$root".into(),
        });
        view.take_events();
        view.on_update(Update::ThreadFailed {
            room_id: "!a".into(),
            root: "$root".into(),
        });
        assert!(view.take_events().contains(&Event::Opened {
            room: Some("!a".into()),
            thread: None
        }));
        // The room's conversation, which the account follows again.
        view.on_update(Update::Timeline {
            room_id: "!a".into(),
            items: vec![item("room")],
        });
        assert_eq!(view.snapshot().items[0].item, item("room"));
    }

    #[test]
    fn a_closed_room_leaves_no_members_or_writers_behind() {
        let mut view = view();
        view.act(Action::Open { room: "!a".into() });
        view.on_update(Update::Typing {
            room_id: "!a".into(),
            names: vec!["Amy".into()],
        });
        view.on_update(Update::Members {
            room_id: "!a".into(),
            members: vec![mothrix_core::model::Member::default()],
        });
        assert_eq!(view.state.typing, ["Amy"]);
        assert!(view.take_events().contains(&Event::Typing {
            room: "!a".into(),
            names: vec!["Amy".into()]
        }));
        view.act(Action::Close);
        assert!(view.state.typing.is_empty() && view.state.members.is_empty());
    }

    #[test]
    fn messages_go_to_the_open_room_and_blank_ones_nowhere() {
        let mut view = view();
        assert!(view.act(send("hello")).is_empty());
        view.act(Action::Open { room: "!a".into() });
        assert!(view.act(send("  \n")).is_empty());
        assert!(matches!(
            &view.act(send(" hello "))[..],
            [Command::Send { room_id, text, .. }, Command::Typing(false)]
                if room_id == "!a" && text == "hello"
        ));
        assert!(
            act(
                &mut view,
                serde_json::json!({"type": "reply", "event_id": "$1", "text": " "})
            )
            .is_empty()
        );
        assert!(matches!(
            &act(&mut view, serde_json::json!({"type": "edit", "event_id": "$1", "text": "fixed"}))[..],
            [Command::Edit { event_id, text, .. }] if event_id == "$1" && text == "fixed"
        ));
    }

    #[test]
    fn a_vote_goes_in_the_poll_s_own_format() {
        let mut view = view();
        view.act(Action::Open { room: "!a".into() });
        view.on_update(Update::Timeline {
            room_id: "!a".into(),
            items: vec![Item {
                event_id: "$poll".into(),
                kind: ItemKind::Poll(Poll {
                    format: PollFormat::Mixed,
                    ..Poll::default()
                }),
                ..Item::default()
            }],
        });
        assert!(matches!(
            &act(&mut view, serde_json::json!({"type": "vote", "event_id": "$poll", "answer": "x"}))[..],
            [Command::Vote { format: PollFormat::Mixed, answers, .. }] if answers == &["x"]
        ));
        assert!(
            act(
                &mut view,
                serde_json::json!({"type": "vote", "event_id": "$other", "answer": "x"})
            )
            .is_empty()
        );
    }

    #[test]
    fn a_tap_on_an_answer_toggles_it_within_what_the_poll_allows() {
        use mothrix_core::model::PollAnswer;
        let mut view = view();
        view.act(Action::Open { room: "!a".into() });
        let answer = |id: &str, self_voted: bool| PollAnswer {
            id: id.into(),
            self_voted,
            ..PollAnswer::default()
        };
        view.on_update(Update::Timeline {
            room_id: "!a".into(),
            items: vec![Item {
                event_id: "$poll".into(),
                kind: ItemKind::Poll(Poll {
                    max_selections: 2,
                    answers: vec![answer("a", true), answer("b", false), answer("c", false)],
                    ..Poll::default()
                }),
                ..Item::default()
            }],
        });
        let mut tap = |answer: &str| {
            act(
                &mut view,
                serde_json::json!({"type": "vote", "event_id": "$poll", "answer": answer}),
            )
        };
        // A second answer joins the first; the first tapped again goes.
        assert!(matches!(
            &tap("b")[..],
            [Command::Vote { answers, .. }] if answers == &["a", "b"]
        ));
        assert!(matches!(
            &tap("a")[..],
            [Command::Vote { answers, .. }] if answers.is_empty()
        ));
    }

    #[test]
    fn messages_say_what_can_be_done_to_them() {
        let mut view = view();
        view.act(Action::Open { room: "!a".into() });
        let poll = Item {
            event_id: "$poll".into(),
            is_own: true,
            kind: ItemKind::Poll(Poll {
                question: "Lunch?".into(),
                ..Poll::default()
            }),
            ..Item::default()
        };
        let mut file = item("a.png");
        file.event_id = "$file".into();
        file.kind = ItemKind::Media;
        file.can_edit = true;
        view.on_update(Update::Timeline {
            room_id: "!a".into(),
            items: vec![poll, file],
        });
        let items = view.snapshot().items;
        assert!(items[0].can.end_poll && !items[0].can.forward && items[0].can.thread);
        assert_eq!(items[0].copy, "Lunch?");
        assert!(items[1].can.forward && items[1].can.open_media && !items[1].can.edit);
        // A poll is not forwarded, even when asked.
        let forward = |event_id: &str| Action::Forward {
            event_id: event_id.into(),
            room: "work/!b".into(),
        };
        assert!(view.act(forward("$poll")).is_empty());
        assert_eq!(view.act(forward("$file")).len(), 1);

        // Among the pinned messages nothing is written, and no thread opens.
        view.act(Action::OpenPinned);
        view.on_update(Update::Thread {
            room_id: "!a".into(),
            root: PINNED_VIEW.into(),
            items: vec![item("pinned")],
        });
        assert!(!view.snapshot().items[0].can.thread);
        assert!(view.act(send("hello")).is_empty());
    }

    #[test]
    fn what_is_done_to_a_message_needs_an_open_room() {
        let mut view = view();
        for action in [
            serde_json::json!({"type": "reply", "event_id": "$1", "text": "hi"}),
            serde_json::json!({"type": "edit", "event_id": "$1", "text": "hi"}),
            serde_json::json!({"type": "delete", "event_id": "$1"}),
            serde_json::json!({"type": "react", "event_id": "$1", "key": "👍"}),
            serde_json::json!({"type": "toggle_pin", "event_id": "$1"}),
            serde_json::json!({"type": "open_media", "event_id": "$1"}),
        ] {
            assert!(act(&mut view, action.clone()).is_empty(), "{action}");
        }
        view.act(Action::Open { room: "!a".into() });
        // Only people still named in the text are mentioned.
        let amy = serde_json::json!({"user_id": "@amy:x", "name": "Amy"});
        let ben = serde_json::json!({"user_id": "@ben:x", "name": "Ben"});
        assert!(matches!(
            &act(&mut view, serde_json::json!({
                "type": "edit", "event_id": "$1", "text": "hi @Amy", "mentions": [amy, ben]
            }))[..],
            [Command::Edit { mentions, .. }] if mentions.len() == 1 && mentions[0].name == "Amy"
        ));
    }

    #[test]
    fn account_commands_go_to_the_account_named() {
        let mut view = view();
        view.act(Action::Open { room: "!a".into() });
        assert!(matches!(
            &view.act(Action::VerifyDevice { account: 1 })[..],
            [Command::VerifyStart]
        ));
        assert_eq!(view.active_account(), 1);
        // The next action is about the open room again.
        view.act(Action::Typing { active: false });
        assert_eq!(view.active_account(), 0);
        assert!(view.act(Action::VerifyDevice { account: 7 }).is_empty());
    }

    /// Samples of what the Swift side decodes, from the real types:
    /// `MOTHRIX_WRITE_FIXTURES=1 cargo test -p mothrix-ffi` writes them to
    /// `ios/MothrixTests/Fixtures`, where the app's tests read them.
    #[test]
    fn what_the_swift_side_decodes() {
        let mut view = view();
        view.on_update(Update::Rooms(vec![
            RoomEntry {
                unread: 2,
                mentions: 1,
                is_dm: true,
                ..room("!a", 0)
            },
            room("work/!b", 1),
        ]));
        view.act(Action::Open { room: "!a".into() });
        let poll = Poll {
            question: "Lunch?".into(),
            answers: vec![
                mothrix_core::model::PollAnswer {
                    id: "a".into(),
                    text: "Pizza".into(),
                    ..Default::default()
                },
                mothrix_core::model::PollAnswer {
                    id: "b".into(),
                    text: "Soup".into(),
                    ..Default::default()
                },
            ],
            ..Poll::default()
        };
        view.on_update(Update::Timeline {
            room_id: "!a".into(),
            items: vec![
                Item {
                    event_id: "$1".into(),
                    sender_id: "@amy:example.org".into(),
                    sender_name: "Amy".into(),
                    ts_ms: 1_763_500_000_000,
                    reactions: vec![mothrix_core::model::Reaction {
                        key: "👍".into(),
                        count: 2,
                        mine: true,
                        ..Default::default()
                    }],
                    ..item(
                        "**Hello** _there_, see https://example.org\n\n```rust\nfn main() {}\n```",
                    )
                },
                Item {
                    event_id: "$2".into(),
                    is_own: true,
                    reply: Some(("Amy".into(), "Hello there".into())),
                    thread: Some((3, Some(("Ben".into(), "me too".into())))),
                    ..item("Yes!")
                },
                Item {
                    event_id: "$3".into(),
                    kind: ItemKind::Media,
                    image: true,
                    ..item("photo.jpg")
                },
                Item {
                    event_id: "$4".into(),
                    kind: ItemKind::Poll(poll),
                    ..item("Lunch?")
                },
                Item {
                    kind: ItemKind::DateDivider,
                    ..item("Today")
                },
            ],
        });
        view.problems = vec![Problem {
            profile: "old".into(),
            text: "old: its store could not be opened".into(),
        }];
        // What came of the above (the rooms, the conversation, what is
        // open), and one of every other kind.
        let mut events = view.take_events();
        let mut failing = account("school");
        failing.sync = SyncStatus::Error("no answer".into());
        let verify = |state| Event::Verify { state };
        events.extend([
            Event::Accounts {
                accounts: vec![account("default"), failing],
            },
            Event::Sidebar {
                lines: view.lines(),
            },
            Event::Picture {
                event_id: "$3".into(),
                url: "data:image/png;base64,AQID".into(),
            },
            Event::LinkPreview {
                url: "https://example.org".into(),
                title: "Example".into(),
                description: "A domain for examples.".into(),
            },
            Event::Typing {
                room: "!a".into(),
                names: vec!["Amy".into()],
            },
            Event::Notice {
                text: "Open a room first.".into(),
            },
            Event::Settings(view.settings()),
            verify(Some(VerifyState::Incoming {
                device: "Amy's phone".into(),
            })),
            verify(Some(VerifyState::Waiting)),
            verify(Some(VerifyState::Emojis(vec![
                ("🐶".into(), "Dog".into()),
                ("🔑".into(), "Key".into()),
            ]))),
            verify(Some(VerifyState::Confirming)),
            verify(Some(VerifyState::Done)),
            verify(Some(VerifyState::Failed("the emoji did not match".into()))),
            verify(None),
            Event::RecoveryKey {
                key: "EsTc 5Sv9 fixt ure0".into(),
            },
            Event::RoomDetails {
                details: RoomDetails {
                    room_id: "!a".into(),
                    name: "Lunch".into(),
                    topic: "Where to".into(),
                    alias: "#lunch:example.org".into(),
                    encrypted: true,
                    members: vec![mothrix_core::model::RoomMember {
                        user_id: "@amy:example.org".into(),
                        name: "Amy".into(),
                        role: "admin".into(),
                        invited: false,
                    }],
                    can_invite: true,
                    ..RoomDetails::default()
                },
            },
            Event::SignIn(SignIn {
                busy: false,
                lines: vec!["Signing in…".into()],
                error: Some("password sign-in failed".into()),
            }),
            Event::Members {
                room: "!a".into(),
                members: vec![Member {
                    user_id: "@amy:example.org".into(),
                    name: "Amy".into(),
                }],
            },
            Event::SearchResults {
                query: "lunch".into(),
                hits: vec![SearchHit {
                    room_id: "!a".into(),
                    room_name: "Lunch".into(),
                    event_id: "$1".into(),
                    sender: "Amy".into(),
                    ts_ms: 1_763_500_000_000,
                    text: "lunch at one?".into(),
                }],
            },
            Event::Reveal {
                event_id: "$1".into(),
            },
            Event::Media {
                event_id: "$3".into(),
                path: "/tmp/photo.jpg".into(),
            },
            Event::MediaKept {
                event_id: "$4".into(),
                path: "/tmp/a1b2c3-invoice.html".into(),
                name: "invoice.html".into(),
            },
            Event::NotSent {
                room: "!a".into(),
                thread: Some("$1".into()),
                text: "written in a thread".into(),
            },
            Event::NotSent {
                room: "work/!b".into(),
                thread: None,
                text: "written in a room".into(),
            },
            Event::Favourites {
                emoji: view.favourites(),
            },
            Event::Problems {
                problems: view.problems.clone(),
            },
        ]);
        // Every kind of event has to be among them. A new variant does
        // not compile here until it has a number: give it the next one,
        // raise `KINDS`, and add a sample above (and its decoding in
        // `ios/App/Models.swift`).
        const KINDS: usize = 22;
        let kind = |event: &Event| match event {
            Event::Accounts { .. } => 0,
            Event::Rooms { .. } => 1,
            Event::Sidebar { .. } => 2,
            Event::Opened { .. } => 3,
            Event::Timeline { .. } => 4,
            Event::Picture { .. } => 5,
            Event::LinkPreview { .. } => 6,
            Event::Typing { .. } => 7,
            Event::Notice { .. } => 8,
            Event::Settings(_) => 9,
            Event::Verify { .. } => 10,
            Event::RecoveryKey { .. } => 11,
            Event::RoomDetails { .. } => 12,
            Event::SignIn(_) => 13,
            Event::Members { .. } => 14,
            Event::SearchResults { .. } => 15,
            Event::Reveal { .. } => 16,
            Event::Media { .. } => 17,
            Event::Favourites { .. } => 18,
            Event::Problems { .. } => 19,
            Event::MediaKept { .. } => 20,
            Event::NotSent { .. } => 21,
        };
        let sampled: std::collections::BTreeSet<usize> = events.iter().map(kind).collect();
        assert_eq!(
            sampled,
            (0..KINDS).collect(),
            "a sample of every kind of event"
        );
        let mut events: Vec<serde_json::Value> = events
            .iter()
            .map(|event| serde_json::to_value(event).unwrap())
            .collect();
        // What leaves as an effect.
        events.push(notify_event("work/!b", "Standup"));
        let snapshot = serde_json::json!({ "type": "snapshot", "snapshot": view.snapshot() });
        for event in &events {
            assert!(event["type"].is_string(), "{event}");
        }
        if std::env::var_os("MOTHRIX_WRITE_FIXTURES").is_some() {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../ios/MothrixTests/Fixtures");
            std::fs::create_dir_all(&dir).unwrap();
            let pretty = |value: &serde_json::Value| serde_json::to_string_pretty(value).unwrap();
            std::fs::write(dir.join("snapshot.json"), pretty(&snapshot)).unwrap();
            std::fs::write(
                dir.join("events.json"),
                pretty(&serde_json::Value::Array(events)),
            )
            .unwrap();
        }
    }

    #[test]
    fn closing_the_room_tells_its_account_and_the_app() {
        let mut view = view();
        view.act(Action::Open {
            room: "work/!b".into(),
        });
        view.take_events();
        assert!(matches!(&view.act(Action::Close)[..], [Command::Close]));
        assert_eq!(view.active_account(), 1);
        assert!(view.take_events().contains(&Event::Opened {
            room: None,
            thread: None
        }));
        assert!(view.act(Action::Close).is_empty());
    }

    #[test]
    fn a_downloaded_attachment_goes_to_the_app_and_forwarding_needs_an_open_room() {
        let mut view = view();
        assert!(
            act(
                &mut view,
                serde_json::json!({"type": "forward", "event_id": "$1", "room": "!a"})
            )
            .is_empty()
        );
        view.act(Action::Open { room: "!a".into() });
        assert!(matches!(
            &act(
                &mut view,
                serde_json::json!({"type": "forward", "event_id": "$1", "room": "!a"})
            )[..],
            [Command::Forward { .. }]
        ));
        view.take_events();
        view.on_update(Update::MediaFile {
            event_id: "$1".into(),
            path: "/tmp/x.jpg".into(),
        });
        assert_eq!(
            view.take_events(),
            [Event::Media {
                event_id: "$1".into(),
                path: "/tmp/x.jpg".into()
            }]
        );
        // One of a kind that is not opened: the app is told too (its
        // row would else say "fetching" for ever), and the notice names
        // the file as its sender did.
        view.on_update(Update::MediaKept {
            event_id: "$2".into(),
            path: "/tmp/a1b2c3-invoice.html".into(),
            name: "invoice.html".into(),
        });
        let events = view.take_events();
        assert!(
            matches!(
                &events[..],
                [Event::Notice { text }, Event::MediaKept { event_id, name, .. }]
                    if text.starts_with("invoice.html was saved") && event_id == "$2" && name == "invoice.html"
            ),
            "{events:?}"
        );
    }

    #[test]
    fn a_voice_message_goes_to_the_open_room() {
        let mut view = view();
        let voice = serde_json::json!({"type": "send_voice", "path": "/tmp/v.m4a",
            "duration_ms": 4200, "waveform": [0.1, 0.9]});
        assert!(act(&mut view, voice.clone()).is_empty());
        view.act(Action::Open { room: "!a".into() });
        assert!(matches!(
            &act(&mut view, voice)[..],
            [Command::SendVoice { path, duration_ms: 4200, waveform, .. }]
                if path == "/tmp/v.m4a" && waveform.len() == 2
        ));
    }

    #[test]
    fn marking_read_and_revealing_go_to_the_backend() {
        let mut view = view();
        assert!(matches!(
            &act(&mut view, serde_json::json!({"type": "mark_read", "room": "work/!b"}))[..],
            [Command::MarkRead { room_id }] if room_id == "work/!b"
        ));
        assert!(
            act(
                &mut view,
                serde_json::json!({"type": "reveal", "event_id": "$1"})
            )
            .is_empty()
        );
        view.act(Action::Open { room: "!a".into() });
        assert!(matches!(
            &act(&mut view, serde_json::json!({"type": "reveal", "event_id": "$1"}))[..],
            [Command::Reveal { event_id }] if event_id == "$1"
        ));
    }

    #[test]
    fn a_push_token_becomes_a_pusher_of_the_account_named() {
        let mut view = view();
        let commands = act(
            &mut view,
            serde_json::json!({
                "type": "register_push", "account": 1, "token": "ab12",
                "app_id": "de.tmrk.mothrix.dev", "gateway": "https://push.tmrk.de/_matrix/push/v1/notify",
                "device": "iPhone", "lang": "de",
            }),
        );
        let [Command::SetPusher(target)] = &commands[..] else {
            panic!("{commands:?}");
        };
        assert_eq!(view.active_account(), 1);
        assert_eq!(target.pushkey, "ab12");
        assert_eq!(target.profile, view.state.accounts[1].profile);
        assert_eq!(target.app_id, "de.tmrk.mothrix.dev");
    }

    #[test]
    fn closing_a_running_verification_cancels_it() {
        let mut view = view();
        view.on_update(Update::Verify(Some(VerifyState::Emojis(Vec::new()))));
        assert!(matches!(
            &view.act(Action::VerifyClose)[..],
            [Command::Verify(VerifyInput::Cancel)]
        ));
        view.on_update(Update::Verify(Some(VerifyState::Done)));
        assert!(view.act(Action::VerifyClose).is_empty());
        assert!(view.take_events().contains(&Event::Verify { state: None }));
        assert!(matches!(
            &act(
                &mut view,
                serde_json::json!({"type": "verify_answer", "answer": "confirm"})
            )[..],
            [Command::Verify(VerifyInput::Confirm)]
        ));
    }

    #[test]
    fn the_room_list_follows_the_config() {
        let config: Config = toml::from_str(
            r#"
            mixed_list = true
            account = "work"
            folded = ["work:Family"]
            [account_names]
            work = "Job"
            [[sections]]
            name = "Family"
            rooms = ["work/!b"]
            "#,
        )
        .unwrap();
        let view = view_with(&config);
        assert_eq!(view.state.accounts[1].label, "Job");
        let lines = view.snapshot().lines;
        // Only the work account, its own section folded.
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].label, "Family");
        assert!(lines[0].folded);
        assert_eq!(lines[0].target, Target::Fold("work:Family".into()));
    }

    #[test]
    fn folding_and_sections_are_saved_and_redrawn() {
        let mut view = view();
        view.act(Action::ShowAccount { account: Some(0) });
        view.take_events();
        view.act(Action::MoveToSection {
            room: "!a".into(),
            section: Some("Family".into()),
        });
        assert!(view.take_effects().contains(&Effect::SaveConfig));
        let events = view.take_events();
        // The mixed list came on, for the section to show.
        assert!(events.iter().any(|event| matches!(
            event,
            Event::Settings(settings) if settings.prefs.mixed_list && settings.sections == ["Family"]
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            Event::Sidebar { lines } if lines[0].row == Row::Header { title: "Family".into() }
        )));
        view.act(Action::Fold {
            key: "Family".into(),
        });
        assert_eq!(view.take_effects(), [Effect::SaveConfig]);
        assert!(view.shown().folded.contains("Family"));
        // Out of it again: the emptied section goes.
        view.act(Action::MoveToSection {
            room: "!a".into(),
            section: None,
        });
        assert!(view.shown().sections.is_empty());
        // A name that cannot be is refused.
        view.act(Action::MoveToSection {
            room: "!a".into(),
            section: Some("Pinned".into()),
        });
        assert!(view.shown().sections.is_empty());
        assert!(
            view.take_events()
                .iter()
                .any(|event| matches!(event, Event::Notice { .. }))
        );
    }

    #[test]
    fn new_messages_notify_unless_their_room_is_in_view() {
        let mut view = view();
        view.state.started = Instant::now() - NOTIFICATION_WARMUP;
        view.act(Action::Open { room: "!a".into() });
        view.take_effects();
        let mut rooms = vec![room("!a", 0), room("work/!b", 1)];
        rooms[0].unread = 1;
        rooms[1].unread = 1;
        rooms[1].name = "Standup".into();
        view.on_update(Update::Rooms(rooms.clone()));
        let notified: Vec<Effect> = view
            .take_effects()
            .into_iter()
            .filter(|effect| matches!(effect, Effect::Notify { .. }))
            .collect();
        assert_eq!(
            notified,
            [Effect::Notify {
                room: "work/!b".into(),
                title: "Standup".into()
            }]
        );
        // With the app in the background, the open room counts too.
        view.focus(false);
        rooms[0].unread = 2;
        view.on_update(Update::Rooms(rooms));
        assert!(view.take_effects().contains(&Effect::Notify {
            room: "!a".into(),
            title: "!a".into()
        }));
    }

    #[test]
    fn nothing_notifies_while_the_counts_catch_up_or_when_switched_off() {
        let mut view = view();
        let mut rooms = vec![room("!a", 0)];
        rooms[0].unread = 5;
        view.on_update(Update::Rooms(rooms.clone()));
        assert!(
            !view
                .take_effects()
                .iter()
                .any(|e| matches!(e, Effect::Notify { .. }))
        );
        view.state.started = Instant::now() - NOTIFICATION_WARMUP;
        view.state.prefs.notifications = false;
        rooms[0].unread = 6;
        view.on_update(Update::Rooms(rooms));
        assert!(
            !view
                .take_effects()
                .iter()
                .any(|e| matches!(e, Effect::Notify { .. }))
        );
    }

    #[test]
    fn pictures_arrive_as_data_addresses_unless_switched_off() {
        let mut view = view();
        let preview = Preview {
            png: vec![1, 2, 3],
            ..Preview::default()
        };
        view.on_update(Update::Preview {
            event_id: "$1".into(),
            preview: preview.clone(),
        });
        assert_eq!(
            view.snapshot().pictures.get("$1").map(String::as_str),
            Some("data:image/png;base64,AQID")
        );
        view.act(Action::Prefs {
            prefs: Prefs {
                images: "off".into(),
                ..Prefs::default()
            },
        });
        view.on_update(Update::Preview {
            event_id: "$2".into(),
            preview,
        });
        assert!(view.snapshot().pictures.is_empty());
    }

    #[test]
    fn a_room_joined_opens_once_it_is_listed() {
        let mut view = view();
        view.on_update(Update::GoTo("!new".into()));
        assert!(view.take_commands().is_empty());
        view.on_update(Update::Rooms(vec![room("!a", 0), room("!new", 0)]));
        assert!(matches!(&view.take_commands()[..], [Command::Open(id)] if id == "!new"));
    }

    #[test]
    fn signing_in_names_a_free_profile_and_adds_the_account() {
        let mut view = view();
        act(
            &mut view,
            serde_json::json!({"type": "sign_in", "server": "@amy:work.example",
                "user": "", "password": "pw", "name": ""}),
        );
        assert_eq!(
            view.take_effects(),
            [Effect::SignIn(accounts::SignIn {
                profile: "work2".into(),
                label: String::new(),
                server: "@amy:work.example".into(),
                user: "@amy:work.example".into(),
                password: "pw".into(),
            })]
        );
        assert!(view.snapshot().sign_in.busy);
        view.sign_in_done(Ok(account("work2")));
        assert_eq!(view.state.accounts.len(), 3);
        assert_eq!(view.take_effects(), [Effect::SaveConfig]);
        // A name typed that is taken is not used.
        view.act(Action::SignIn {
            server: "matrix.org".into(),
            user: "amy".into(),
            password: "pw".into(),
            name: "work".into(),
        });
        assert!(view.snapshot().sign_in.error.is_some());
        assert!(view.take_effects().is_empty());
    }

    #[test]
    fn a_signed_out_account_takes_its_rooms_along() {
        let mut view = view();
        view.act(Action::Open {
            room: "work/!b".into(),
        });
        view.act(Action::ShowAccount { account: Some(1) });
        let closed = view.state.forget_account(1);
        assert!(closed);
        view.account_gone(closed);
        assert_eq!(view.state.accounts.len(), 1);
        assert_eq!(view.state.rooms, [room("!a", 0)]);
        assert_eq!(view.snapshot().open_room, None);
        assert_eq!(view.state.account_filter, None);
        // The last one can go too: the app then asks to sign in.
        view.state.forget_account(0);
        assert!(view.state.accounts.is_empty() && view.state.rooms.is_empty());
    }

    #[test]
    fn links_are_looked_up_once_and_only_when_wanted() {
        let mut view = view();
        let url = || Action::PreviewLink {
            url: "https://x.example".into(),
        };
        assert!(view.act(url()).is_empty());
        view.state.prefs.link_previews = true;
        // No room is open: there is no one to ask.
        assert!(view.act(url()).is_empty());
        view.act(Action::Open { room: "!a".into() });
        assert!(matches!(
            &view.act(url())[..],
            [Command::PreviewLink { room_id, .. }] if room_id == "!a"
        ));
        assert!(view.act(url()).is_empty());
        // The app saw the link in one room, and another is open here by
        // now: not asked, not of the other room's account, and no word.
        view.act(Action::Open {
            room: "work/!b".into(),
        });
        view.take_events();
        assert!(view.act_in(url(), "!a", None).is_empty());
        assert!(view.take_events().is_empty());
        // In the room shown it is asked anew (also from a thread of it):
        // what was asked about is each room's own.
        assert!(matches!(
            &view.act_in(url(), "work/!b", Some("$root"))[..],
            [Command::PreviewLink { room_id, .. }] if room_id == "work/!b"
        ));
    }

    #[test]
    fn own_sections_are_renamed_moved_and_removed_with_their_subsections() {
        let config: Config = toml::from_str(
            r#"
            [[sections]]
            name = "School"
            rooms = ["!a"]
            [[sections]]
            name = "School/Maths"
            rooms = ["!m"]
            [[sections]]
            name = "Family"
            rooms = ["!f"]
            "#,
        )
        .unwrap();
        let mut view = view_with(&config);
        let names = |view: &View| view.settings().sections;
        view.act(Action::Fold {
            key: "School/Maths".into(),
        });
        view.act(Action::RenameSection {
            from: "School".into(),
            to: "Uni".into(),
        });
        assert_eq!(names(&view), ["Uni", "Uni/Maths", "Family"]);
        assert!(view.shown().folded.contains("Uni/Maths"));
        // Taken, regardless of case.
        view.act(Action::RenameSection {
            from: "Uni".into(),
            to: "family".into(),
        });
        assert_eq!(names(&view), ["Uni", "Uni/Maths", "Family"]);
        // Among its siblings: subsections travel with their parent.
        view.act(Action::MoveSection {
            name: "Family".into(),
            down: false,
        });
        assert_eq!(names(&view), ["Family", "Uni", "Uni/Maths"]);
        view.act(Action::RemoveSection { name: "Uni".into() });
        assert_eq!(names(&view), ["Family"]);
        assert!(view.shown().folded.is_empty());
        assert!(view.take_effects().contains(&Effect::SaveConfig));
    }

    #[test]
    fn every_kind_of_line_reaches_the_app() {
        let mut view = view();
        view.act(Action::Fold {
            key: "work:ROOMS".into(),
        });
        let json = serde_json::to_value(view.snapshot()).unwrap();
        let rows: Vec<&str> = json["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line["row"]["type"].as_str().unwrap())
            .collect();
        assert_eq!(rows, ["account", "header", "room", "account", "folded"]);
        assert_eq!(json["lines"][1]["row"]["title"], "ROOMS");
        let json = serde_json::to_value(Event::Settings(view.settings())).unwrap();
        assert_eq!(json["type"], "settings");
        assert_eq!(json["prefs"]["sort"], "recent");
    }

    #[test]
    fn search_finds_here_or_everywhere_and_a_hit_opens_where_it_is() {
        let mut view = view();
        assert!(
            view.act(Action::Search {
                text: "x".into(),
                everywhere: false
            })
            .is_empty()
        );
        assert!(matches!(
            &view.act(Action::Search { text: " lunch ".into(), everywhere: true })[..],
            [Command::Search { query, room_id: None }] if query == "lunch"
        ));
        view.act(Action::Open { room: "!a".into() });
        assert!(matches!(
            &view.act(Action::Search { text: "x".into(), everywhere: false })[..],
            [Command::Search { room_id: Some(room), .. }] if room == "!a"
        ));
        assert!(matches!(
            &view.act(Action::ShowHit { room: "work/!b".into(), event_id: "$9".into() })[..],
            [Command::Open(room), Command::Reveal { event_id }] if room == "work/!b" && event_id == "$9"
        ));
        view.on_update(Update::Reveal("$9".into()));
        assert!(view.take_events().contains(&Event::Reveal {
            event_id: "$9".into()
        }));
    }

    #[test]
    fn a_hit_whose_room_does_not_open_is_not_looked_for_in_the_open_one() {
        let mut view = view();
        let mut rooms = view.state.rooms.clone();
        rooms.push(RoomEntry {
            is_invite: true,
            ..room("!party", 0)
        });
        view.on_update(Update::Rooms(rooms));
        view.act(Action::Open { room: "!a".into() });
        let hit = |room: &str| Action::ShowHit {
            room: room.into(),
            event_id: "$9".into(),
        };
        // No longer listed, and an invitation (which is only asked about).
        assert!(view.act(hit("!gone")).is_empty());
        assert!(view.act(hit("!party")).is_empty());
        assert_eq!(view.snapshot().open_room.as_deref(), Some("!a"));
        // In the open room itself: only looked for.
        assert!(matches!(
            &view.act(hit("!a"))[..],
            [Command::Reveal { event_id }] if event_id == "$9"
        ));
    }

    #[test]
    fn an_invitation_is_asked_about_when_opened_and_joined_by_the_app_s_own_answer() {
        let mut view = view();
        let mut rooms = view.state.rooms.clone();
        rooms.push(RoomEntry {
            is_invite: true,
            name: "Party".into(),
            ..room("!party", 0)
        });
        view.on_update(Update::Rooms(rooms));
        view.take_events();
        // A tap on the notification "X invited you" joins nothing.
        assert!(view.notification_clicked("!party".into()).is_empty());
        assert!(view.take_events().iter().any(|event| matches!(
            event, Event::Notice { text } if text.contains("Party") && text.contains("invitation")
        )));
        assert_eq!(view.snapshot().open_room, None);
        // "Join" in the room list's own question: one step.
        let mut view = self::view();
        let mut rooms = view.state.rooms.clone();
        rooms.push(RoomEntry {
            is_invite: true,
            ..room("!party", 0)
        });
        view.on_update(Update::Rooms(rooms));
        view.take_events();
        assert!(matches!(
            &act(&mut view, serde_json::json!({"type": "accept_invitation", "room": "!party"}))[..],
            [Command::Open(room)] if room == "!party"
        ));
        assert_eq!(view.snapshot().open_room.as_deref(), Some("!party"));
        assert!(
            !view
                .take_events()
                .iter()
                .any(|event| matches!(event, Event::Notice { .. }))
        );
    }

    #[test]
    fn what_was_begun_in_one_conversation_is_not_sent_into_another() {
        let mut view = view();
        view.act(Action::Open { room: "!a".into() });
        view.take_events();
        let poll = || Action::CreatePoll {
            question: "Lunch?".into(),
            answers: vec!["Pizza".into(), "Soup".into()],
            multiple: false,
            hidden: false,
        };
        assert_eq!(view.act_in(poll(), "!a", None).len(), 1);
        // A thread of the room was entered meanwhile.
        view.act(Action::OpenThread {
            event_id: "$root".into(),
        });
        assert!(view.act_in(poll(), "!a", None).is_empty());
        assert!(view.act_in(send("hi"), "!a", None).is_empty());
        assert!(matches!(
            &view.act_in(send("hi"), "!a", Some("$root"))[..],
            [Command::Send { thread: Some(thread), .. }, ..] if thread == "$root"
        ));
        // Another room was opened meanwhile (a notification tapped).
        view.take_events();
        view.notification_clicked("work/!b".into());
        assert!(view.act_in(poll(), "!a", Some("$root")).is_empty());
        assert!(view.act_in(send("hi"), "!a", None).is_empty());
        let events = view.take_events();
        assert!(events.iter().any(|event| matches!(
            event, Event::Notice { text } if text.starts_with("Not sent")
        )));
        // The app emptied its message box: the text goes back, once, for
        // the conversation it was written in. (A poll's sheet is still
        // open, and holds what was typed.)
        let back: Vec<&Event> = events
            .iter()
            .filter(|event| matches!(event, Event::NotSent { .. }))
            .collect();
        assert_eq!(
            back,
            [&Event::NotSent {
                room: "!a".into(),
                thread: None,
                text: "hi".into()
            }]
        );
        assert!(view.take_effects().is_empty());
        // The account refuses one too, when it follows another
        // conversation than the app shows (a thread that did not open).
        view.on_update(Update::NotSent {
            room_id: "work/!b".into(),
            thread: Some("$root".into()),
            text: "in the thread".into(),
        });
        assert!(view.take_events().contains(&Event::NotSent {
            room: "work/!b".into(),
            thread: Some("$root".into()),
            text: "in the thread".into()
        }));
        // A file and a voice message name the conversation they go into.
        let voice = Action::SendVoice {
            path: "/tmp/v.m4a".into(),
            duration_ms: 1,
            waveform: Vec::new(),
        };
        assert!(matches!(
            &view.act_in(voice, "work/!b", None)[..],
            [Command::SendVoice { room_id, thread: None, .. }] if room_id == "work/!b"
        ));
        let file = Action::Upload {
            name: "a.txt".into(),
            data: String::new(),
            caption: None,
        };
        view.act_in(file, "work/!b", None);
        assert!(matches!(
            &view.take_effects()[..],
            [Effect::Upload { room_id, thread: None, .. }] if room_id == "work/!b"
        ));
    }

    #[test]
    fn an_account_that_did_not_start_can_be_removed() {
        let config: Config = toml::from_str(
            r#"
            folded = ["old:ROOMS"]
            [[sections]]
            name = "Family"
            rooms = ["old/!x", "!a"]
            "#,
        )
        .unwrap();
        let mut view = View::new(
            vec![account("default")],
            vec![Problem {
                profile: "old".into(),
                text: "old: its store could not be opened".into(),
            }],
            &config,
        );
        view.take_effects();
        // Only one that is listed as not started: not a running one, and
        // no name the app made up.
        for profile in ["default", "../x", ""] {
            act(
                &mut view,
                serde_json::json!({"type": "remove_account", "profile": profile}),
            );
            assert!(view.take_effects().is_empty(), "{profile}");
        }
        act(
            &mut view,
            serde_json::json!({"type": "remove_account", "profile": "old"}),
        );
        assert_eq!(view.take_effects(), [Effect::Remove("old".into())]);
        // It could not be: it stays listed, and the user reads why.
        view.take_events();
        view.removed("old", Err("another mothrix runs with it".into()));
        assert_eq!(view.snapshot().problems.len(), 1);
        assert!(view.take_events().iter().any(|event| matches!(
            event, Event::Notice { text } if text.contains("another mothrix")
        )));
        // Removed: no longer listed, and its rooms and folds do not wait
        // for the next account of that name.
        // (The server could not be told: that is said with it.)
        let left = "The device is still signed in on the server (no answer): remove it.";
        view.removed("old", Ok(Some(left.into())));
        assert!(view.snapshot().problems.is_empty());
        let events = view.take_events();
        assert!(events.contains(&Event::Problems {
            problems: Vec::new()
        }));
        assert!(events.iter().any(|event| matches!(
            event, Event::Notice { text }
                if text.starts_with("'old' is removed") && text.contains("still signed in")
        )));
        assert_eq!(view.shown().sections[0].rooms, ["!a"]);
        assert!(view.shown().folded.is_empty());
        assert_eq!(view.take_effects(), [Effect::SaveConfig]);
    }

    #[test]
    fn a_poll_needs_a_question_and_two_answers() {
        let mut view = view();
        view.act(Action::Open { room: "!a".into() });
        let poll = |answers: &[&str]| Action::CreatePoll {
            question: "Lunch?".into(),
            answers: answers.iter().map(|answer| (*answer).to_owned()).collect(),
            multiple: true,
            hidden: false,
        };
        assert!(view.act(poll(&["Pizza", "  "])).is_empty());
        assert!(matches!(
            &view.act(poll(&[" Pizza ", "", "Noodles"]))[..],
            [Command::CreatePoll { answers, multiple: true, .. }] if answers == &["Pizza", "Noodles"]
        ));
    }

    #[test]
    fn emoji_taken_move_to_the_front() {
        let mut view = view();
        view.act(Action::Open { room: "!a".into() });
        view.take_events();
        view.act(Action::React {
            event_id: "$1".into(),
            key: "🦋".into(),
        });
        assert!(
            view.take_effects()
                .contains(&Effect::UsedEmoji("🦋".into()))
        );
        assert!(view.take_events().iter().any(|event| matches!(
            event, Event::Favourites { emoji } if emoji[0] == "🦋"
        )));
        assert_eq!(view.snapshot().favourites[0], "🦋");
    }

    #[test]
    fn a_reaction_taken_back_or_of_plain_text_is_no_emoji_taken() {
        let mut view = view();
        view.act(Action::Open { room: "!a".into() });
        view.on_update(Update::Timeline {
            room_id: "!a".into(),
            items: vec![Item {
                event_id: "$1".into(),
                reactions: vec![mothrix_core::model::Reaction {
                    key: "🦋".into(),
                    count: 1,
                    mine: true,
                    ..Default::default()
                }],
                ..item("hi")
            }],
        });
        view.take_events();
        view.take_effects();
        let react = |key: &str| Action::React {
            event_id: "$1".into(),
            key: key.into(),
        };
        // Both still go to the server.
        assert_eq!(view.act(react("🦋")).len(), 1);
        assert_eq!(view.act(react("+1 from me")).len(), 1);
        assert!(view.take_effects().is_empty());
        assert!(
            !view
                .take_events()
                .iter()
                .any(|event| matches!(event, Event::Favourites { .. }))
        );
    }

    #[test]
    fn a_theme_is_picked_by_name_and_saved() {
        let mut view = view();
        assert!(view.settings().themes.len() >= 6);
        view.act(Action::Theme {
            name: "Nord".into(),
        });
        assert_eq!(view.theme_name(), "nord");
        assert!(view.take_effects().contains(&Effect::SaveConfig));
        let json = serde_json::to_value(Event::Settings(view.settings())).unwrap();
        assert_eq!(json["theme"]["background"], "#2e3440");
        view.act(Action::Theme {
            name: "no such".into(),
        });
        assert_eq!(view.theme_name(), "nord");
    }

    #[test]
    fn events_are_tagged_for_the_app() {
        let json = serde_json::to_value(Event::Notice { text: "x".into() }).unwrap();
        assert_eq!(json, serde_json::json!({"type": "notice", "text": "x"}));
        let line = &view().snapshot().lines[0];
        let json = serde_json::to_value(line).unwrap();
        assert_eq!(
            json["row"],
            serde_json::json!({"type": "account", "index": 0})
        );
        assert_eq!(
            json["target"],
            serde_json::json!({"type": "fold", "key": "@default"})
        );
    }
}
