// SPDX-License-Identifier: MPL-2.0

//! Client state: what both front ends (the terminal's `App`, the iPhone
//! app's `View`) know about the accounts and rooms, apart from how they
//! show it. Each holds one, as `state`; [`crate::accounts::Accounts`]
//! serves it.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use crate::{
    accounts::Account,
    config::Shown,
    matrix::Command,
    model::{Item, Member, RoomEntry},
    prefs::Prefs,
    sections::OwnSections,
    sidebar::Folds,
};

/// After the start, unread counts catch up with what came in meanwhile:
/// no notifications for that.
pub const NOTIFICATION_WARMUP: Duration = Duration::from_secs(12);

/// A room with new messages, to tell the user about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    pub room: String,
    pub title: String,
}

/// What came of opening a room ([`ClientState::open`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opening {
    /// It is not listed (yet): nothing happened.
    NotListed,
    /// It was open already; `left_thread`: a thread of it was shown, and
    /// the room is now.
    Again { left_thread: bool },
    /// It is open now (its notifications are read), after `previous`.
    Opened {
        room: String,
        previous: Option<String>,
    },
}

impl Opening {
    /// What the account is to do about it.
    pub fn commands(&self) -> Vec<Command> {
        match self {
            Self::NotListed | Self::Again { left_thread: false } => Vec::new(),
            Self::Again { left_thread: true } => vec![Command::CloseThread],
            Self::Opened { room, .. } => vec![Command::Open(room.clone())],
        }
    }
}

#[derive(Debug)]
pub struct ClientState {
    /// The accounts, in the order shown.
    pub accounts: Vec<Account>,
    /// The rooms of all accounts, known by their keys.
    pub rooms: Vec<RoomEntry>,
    pub prefs: Prefs,
    pub sections: OwnSections,
    pub folded: Folds,
    /// The account shown alone, if any.
    pub account_filter: Option<usize>,
    /// Only rooms with something unread are listed.
    pub unread_only: bool,
    /// The room shown (its key), and the thread in it (its first message,
    /// or [`crate::matrix::PINNED_VIEW`]).
    pub open_room: Option<String>,
    pub open_thread: Option<String>,
    /// The conversation of the open room or thread.
    pub items: Vec<Item>,
    /// The open room's members.
    pub members: Vec<Member>,
    /// Who is writing in the open room.
    pub typing: Vec<String>,
    /// A room to open as soon as it is listed (one just joined).
    pub goto: Option<String>,
    /// Link previews by address (title, description), and the addresses
    /// asked for (once each).
    pub link_previews: HashMap<String, (String, String)>,
    pub link_previews_asked: HashSet<String>,
    /// Whether the user looks at mothrix: the terminal has the focus, the
    /// app is in front.
    pub focused: bool,
    /// Until [`NOTIFICATION_WARMUP`] after this, nothing notifies.
    pub started: Instant,
    /// The account the action at hand names, for commands without a
    /// room (the app's actions can name one).
    pub named_account: Option<usize>,
    /// What the config file keeps of this ([`ClientState::shown`]) changed.
    pub config_dirty: bool,
    /// Unread counts as last seen, by room key.
    last_unread: HashMap<String, u64>,
}

impl Default for ClientState {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            rooms: Vec::new(),
            prefs: Prefs::default(),
            sections: OwnSections::default(),
            folded: Folds::default(),
            account_filter: None,
            unread_only: false,
            open_room: None,
            open_thread: None,
            items: Vec::new(),
            members: Vec::new(),
            typing: Vec::new(),
            goto: None,
            link_previews: HashMap::new(),
            link_previews_asked: HashSet::new(),
            focused: true,
            started: Instant::now(),
            named_account: None,
            config_dirty: false,
            last_unread: HashMap::new(),
        }
    }
}

impl ClientState {
    /// For these accounts, everything else as at the start.
    pub fn new(accounts: Vec<Account>) -> Self {
        Self {
            accounts,
            ..Self::default()
        }
    }

    /// What the config file keeps of it.
    pub fn shown(&self) -> Shown<'_> {
        Shown {
            prefs: &self.prefs,
            accounts: &self.accounts,
            account_filter: self.account_filter,
            sections: &self.sections,
            folded: &self.folded,
        }
    }

    /// The account commands without a room go to: the one the action at
    /// hand names, else the open room's, else the one shown alone, else
    /// the one the front end suggests (`hint`: the terminal's cursor),
    /// else the first.
    pub fn active_account(&self, hint: Option<usize>) -> usize {
        let open = self
            .open_room
            .as_ref()
            .and_then(|key| self.rooms.iter().find(|room| room.id == *key))
            .map(|room| room.account);
        self.named_account
            .or(open)
            .or(self.account_filter)
            .or(hint)
            .filter(|account| *account < self.accounts.len())
            .unwrap_or(0)
    }

    /// Accounts `a` and `b` change places; their rooms go with them.
    pub fn swap_accounts(&mut self, a: usize, b: usize) {
        if a == b || a >= self.accounts.len() || b >= self.accounts.len() {
            return;
        }
        let other = |index: usize| match index {
            index if index == a => b,
            index if index == b => a,
            index => index,
        };
        self.accounts.swap(a, b);
        for room in &mut self.rooms {
            room.account = other(room.account);
        }
        self.rooms.sort_by_key(|room| room.account);
        self.account_filter = self.account_filter.map(other);
        self.config_dirty = true;
    }

    /// The account was signed out: it goes, with its rooms (and the open
    /// room, if it was one of them). Returns whether that closed the open
    /// room.
    pub fn forget_account(&mut self, account: usize) -> bool {
        if account >= self.accounts.len() {
            return false;
        }
        let open = self
            .open_room
            .as_ref()
            .and_then(|key| self.rooms.iter().find(|room| room.id == *key));
        let closed = open.is_some_and(|room| room.account == account);
        if closed {
            self.open_room = None;
            self.open_thread = None;
            self.items.clear();
            self.members.clear();
            self.typing.clear();
        }
        self.accounts.remove(account);
        self.rooms.retain(|room| room.account != account);
        for room in &mut self.rooms {
            if room.account > account {
                room.account -= 1;
            }
        }
        self.account_filter = match self.account_filter {
            Some(shown) if shown == account => None,
            Some(shown) if shown > account => Some(shown - 1),
            other => other,
        };
        self.named_account = None;
        self.config_dirty = true;
        closed
    }

    /// Show a room (by its key), if it is listed; the open one again
    /// leads out of its thread.
    pub fn open(&mut self, room: &str) -> Opening {
        if !self.rooms.iter().any(|known| known.id == room) {
            return Opening::NotListed;
        }
        if self.open_room.as_deref() == Some(room) {
            return Opening::Again {
                left_thread: self.leave_thread(),
            };
        }
        let previous = self.open_room.replace(room.to_owned());
        self.open_thread = None;
        self.items.clear();
        self.members.clear();
        self.typing.clear();
        Opening::Opened {
            room: room.to_owned(),
            previous,
        }
    }

    /// Back from a thread to its room's conversation. Returns whether a
    /// thread was shown ([`Command::CloseThread`] then tells the account).
    pub fn leave_thread(&mut self) -> bool {
        let left = self.open_thread.take().is_some();
        if left {
            self.items.clear();
        }
        left
    }

    /// The room to show now that it is listed, if one is waiting
    /// ([`ClientState::goto`]: a room just joined).
    pub fn take_goto(&mut self) -> Option<String> {
        let room = self.goto.as_ref()?;
        if !self.rooms.iter().any(|known| known.id == *room) {
            return None;
        }
        self.goto.take()
    }

    /// A new room list (of all accounts). Returns the rooms with new
    /// messages to tell the user about: with notifications on, once the
    /// start has settled, unless the room is on screen in front of them.
    pub fn set_rooms(&mut self, rooms: Vec<RoomEntry>) -> Vec<Notification> {
        let settled = self.started.elapsed() >= NOTIFICATION_WARMUP;
        let mut news = Vec::new();
        for room in &rooms {
            let before = self
                .last_unread
                .insert(room.id.clone(), room.unread)
                .unwrap_or(0);
            let watching = self.focused && self.open_room.as_ref() == Some(&room.id);
            if settled && self.prefs.notifications && room.unread > before && !watching {
                news.push(Notification {
                    room: room.id.clone(),
                    title: room.name.clone(),
                });
            }
        }
        self.rooms = rooms;
        news
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(id: &str, unread: u64) -> RoomEntry {
        RoomEntry {
            id: id.into(),
            name: id.to_uppercase(),
            unread,
            ..RoomEntry::default()
        }
    }

    fn rooms(state: &mut ClientState, rooms: &[(&str, u64)]) -> Vec<String> {
        let rooms = rooms.iter().map(|(id, unread)| room(id, *unread)).collect();
        let news = state.set_rooms(rooms);
        news.into_iter().map(|news| news.title).collect()
    }

    fn account(profile: &str) -> Account {
        Account {
            profile: profile.into(),
            label: profile.into(),
            user_id: format!("@me:{profile}"),
            own_name: None,
            sync: crate::matrix::SyncStatus::Connecting,
            verified: None,
        }
    }

    /// Accounts "default" (rooms !a, !b) and "work" (work/!c).
    fn two_accounts() -> ClientState {
        let mut state = ClientState::new(vec![account("default"), account("work")]);
        let mut rooms = vec![room("!a", 0), room("!b", 0), room("work/!c", 0)];
        rooms[2].account = 1;
        state.set_rooms(rooms);
        state
    }

    #[test]
    fn commands_without_a_room_go_to_the_named_then_open_then_shown_account() {
        let mut state = two_accounts();
        assert_eq!(state.active_account(None), 0);
        assert_eq!(state.active_account(Some(1)), 1);
        state.account_filter = Some(1);
        assert_eq!(state.active_account(Some(0)), 1);
        state.open_room = Some("!a".into());
        assert_eq!(state.active_account(Some(1)), 0);
        state.named_account = Some(1);
        assert_eq!(state.active_account(None), 1);
        // One that is not there (any more).
        state.named_account = Some(5);
        assert_eq!(state.active_account(None), 0);
    }

    #[test]
    fn accounts_change_places_and_go_with_their_rooms() {
        let mut state = two_accounts();
        state.account_filter = Some(1);
        state.swap_accounts(0, 1);
        assert_eq!(state.accounts[0].profile, "work");
        let rooms: Vec<_> = state
            .rooms
            .iter()
            .map(|room| (room.id.as_str(), room.account))
            .collect();
        assert_eq!(rooms, [("work/!c", 0), ("!a", 1), ("!b", 1)]);
        assert_eq!(state.account_filter, Some(0));
        assert!(std::mem::take(&mut state.config_dirty));

        // Signing out the one whose room is open closes it.
        state.open_room = Some("work/!c".into());
        state.items.push(Item::default());
        assert!(state.forget_account(0));
        assert_eq!(state.open_room, None);
        assert!(state.items.is_empty());
        assert_eq!(state.account_filter, None);
        let rooms: Vec<_> = state
            .rooms
            .iter()
            .map(|room| (room.id.as_str(), room.account))
            .collect();
        assert_eq!(rooms, [("!a", 0), ("!b", 0)]);
        // The last one can go too.
        assert!(!state.forget_account(0));
        assert!(state.accounts.is_empty() && state.rooms.is_empty());
        assert!(state.config_dirty);
    }

    #[test]
    fn a_listed_room_opens_and_the_open_one_again_leads_out_of_its_thread() {
        let mut state = two_accounts();
        assert_eq!(state.open("!x"), Opening::NotListed);
        assert_eq!(state.open_room, None);
        let opening = state.open("!a");
        assert!(matches!(&opening.commands()[..], [Command::Open(room)] if room == "!a"));
        state.open_thread = Some("$root".into());
        state.items.push(Item::default());
        state.members.push(Member::default());
        // Again: out of the thread, back to the room.
        let again = state.open("!a");
        assert_eq!(again, Opening::Again { left_thread: true });
        assert!(matches!(&again.commands()[..], [Command::CloseThread]));
        assert!(state.items.is_empty() && state.open_thread.is_none());
        assert_eq!(state.open("!a"), Opening::Again { left_thread: false });
        // Another room: what was shown of the first one goes.
        assert_eq!(
            state.open("work/!c"),
            Opening::Opened {
                room: "work/!c".into(),
                previous: Some("!a".into())
            }
        );
        assert!(state.members.is_empty());
    }

    #[test]
    fn a_room_to_go_to_waits_until_it_is_listed() {
        let mut state = two_accounts();
        state.goto = Some("!new".into());
        assert_eq!(state.take_goto(), None);
        let mut rooms = state.rooms.clone();
        rooms.push(room("!new", 0));
        state.set_rooms(rooms);
        assert_eq!(state.take_goto(), Some("!new".into()));
        assert_eq!(state.goto, None);
    }

    #[test]
    fn new_messages_notify_once_the_start_settled_unless_their_room_is_watched() {
        let mut state = ClientState::default();
        // Counts filling in at the start are not news.
        assert!(rooms(&mut state, &[("!a", 2)]).is_empty());
        state.started = Instant::now() - NOTIFICATION_WARMUP;
        assert_eq!(rooms(&mut state, &[("!a", 3), ("!b", 1)]), ["!A", "!B"]);
        // The same counts again, or fewer: nothing.
        assert!(rooms(&mut state, &[("!a", 3), ("!b", 0)]).is_empty());
        assert_eq!(state.rooms.len(), 2);

        // The open room, while the user looks: nothing; once away: news.
        state.open_room = Some("!a".into());
        assert!(rooms(&mut state, &[("!a", 4)]).is_empty());
        state.focused = false;
        assert_eq!(rooms(&mut state, &[("!a", 5)]), ["!A"]);

        // Switched off.
        state.prefs.notifications = false;
        assert!(rooms(&mut state, &[("!a", 6), ("!b", 9)]).is_empty());
    }
}
