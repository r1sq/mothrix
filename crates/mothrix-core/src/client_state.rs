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
    matrix::{Command, Update},
    model::{Item, Member, RoomEntry},
    prefs::Prefs,
    sections::OwnSections,
    sidebar::Folds,
};

/// After the start, unread counts catch up with what came in meanwhile:
/// no notifications for that.
pub const NOTIFICATION_WARMUP: Duration = Duration::from_secs(12);

/// How long after an invitation was asked about opening it again joins
/// it: the second Enter or tap of someone who means it comes soon, and a
/// slip an hour later is asked again.
pub const INVITATION_ASKED: Duration = Duration::from_secs(60);

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
    /// It is an invitation (of this name), and opening it would accept
    /// it: nothing happened but that the user is to be asked. Opening it
    /// again soon ([`INVITATION_ASKED`]) joins.
    Invitation { name: String },
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
            Self::NotListed | Self::Invitation { .. } | Self::Again { left_thread: false } => {
                Vec::new()
            }
            Self::Again { left_thread: true } => vec![Command::CloseThread],
            Self::Opened { room, .. } => vec![Command::Open(room.clone())],
        }
    }
}

/// What an update did to the client state ([`ClientState::take`]).
#[derive(Debug)]
pub enum Taken {
    /// The rooms are listed anew. `news`: the rooms to notify about;
    /// `show`: a room that waited to be shown ([`ClientState::goto`]) and
    /// is listed now.
    Rooms {
        news: Vec<Notification>,
        show: Option<String>,
    },
    /// A room to show now: the account says to go there, and it is listed.
    Show(String),
    /// The conversation shown has other messages now.
    Items,
    /// Other people are writing in the open room.
    Typing,
    /// The open room's members are known (anew).
    Members,
    /// A link preview came in, for this address.
    LinkPreview(String),
    /// The thread shown could not be opened: the room's conversation is
    /// shown again (the account went back to it by itself).
    LeftThread,
    /// It was about a room or thread that is not shown (any more), or a
    /// room to go to that is not listed yet: nothing to show.
    Nothing,
    /// Not about the rooms and the conversation: for the front end.
    Other(Update),
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
    /// When each account's rooms were first seen, by profile; `None`:
    /// in the first list, with the program's start.
    accounts_seen: HashMap<String, Option<Instant>>,
    /// The account the action at hand names, for commands without a
    /// room (the app's actions can name one).
    pub named_account: Option<usize>,
    /// What the config file keeps of this ([`ClientState::shown`]) changed.
    pub config_dirty: bool,
    /// Unread counts as last seen, by room key.
    last_unread: HashMap<String, u64>,
    /// The invitation the user was just asked about, and when (see
    /// [`ClientState::open`]).
    invite_asked: Option<(String, Instant)>,
    /// A room a command of the user asked for (joined, made, a chat
    /// started): should the list still call it an invitation when it
    /// opens, there is nothing left to ask.
    wanted: Option<String>,
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
            accounts_seen: HashMap::new(),
            named_account: None,
            config_dirty: false,
            last_unread: HashMap::new(),
            invite_asked: None,
            wanted: None,
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
        let closed = open.is_some_and(|room| room.account == account) && self.close();
        // What was known of it for notifying goes too: an account signed
        // in under its profile's name later in this run is a new one,
        // whose unread rooms are no news ([`ClientState::set_rooms`]).
        let gone = self.accounts.remove(account);
        self.accounts_seen.remove(&gone.profile);
        for room in self.rooms.iter().filter(|room| room.account == account) {
            self.last_unread.remove(&room.id);
        }
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
    ///
    /// An invitation is accepted by opening it ([`Command::Open`] joins),
    /// so the first time only says so ([`Opening::Invitation`]) and
    /// opening it again soon joins: a slip of the hand, or a tap on the
    /// notification "X invited you", would else join a stranger's room.
    pub fn open(&mut self, room: &str) -> Opening {
        let Some(known) = self.rooms.iter().find(|known| known.id == room) else {
            return Opening::NotListed;
        };
        let asked = self
            .invite_asked
            .take()
            .is_some_and(|(asked, at)| asked == room && at.elapsed() < INVITATION_ASKED);
        let wanted = self.wanted.take_if(|wanted| wanted == room).is_some();
        if known.is_invite && !asked && !wanted {
            let name = known.name.clone();
            self.invite_asked = Some((room.to_owned(), Instant::now()));
            return Opening::Invitation { name };
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

    /// When an invitation was asked about, if one was and has not been
    /// opened again yet.
    pub fn invitation_asked(&self) -> Option<Instant> {
        self.invite_asked.as_ref().map(|(_, at)| *at)
    }

    /// The user went on to something else: opening the invitation asked
    /// about asks again.
    pub fn forget_invitation(&mut self) {
        self.invite_asked = None;
    }

    /// Show no room. Returns whether one was shown.
    pub fn close(&mut self) -> bool {
        self.open_thread = None;
        self.items.clear();
        self.members.clear();
        self.typing.clear();
        self.open_room.take().is_some()
    }

    /// Show the thread that starts at this message of the open room (or
    /// [`crate::matrix::PINNED_VIEW`]), also from another thread. Returns
    /// whether it is shown now and was not before
    /// ([`Command::OpenThread`] then tells the account).
    pub fn enter_thread(&mut self, root: &str) -> bool {
        if self.open_room.is_none() || root.is_empty() || self.open_thread.as_deref() == Some(root)
        {
            return false;
        }
        self.open_thread = Some(root.to_owned());
        self.items.clear();
        true
    }

    /// Whether the open room's pinned messages are shown: nothing can be
    /// written there.
    pub fn pinned_view(&self) -> bool {
        self.open_thread.as_deref() == Some(crate::matrix::PINNED_VIEW)
    }

    /// Take in what an account says about the rooms and the conversation
    /// shown; news of rooms and threads that are not shown is dropped.
    /// Everything else comes back as [`Taken::Other`].
    pub fn take(&mut self, update: Update) -> Taken {
        let shown = |state: &Self, room: &str| state.open_room.as_deref() == Some(room);
        match update {
            Update::Rooms(rooms) => {
                let news = self.set_rooms(rooms);
                Taken::Rooms {
                    news,
                    show: self.take_goto(),
                }
            }
            Update::GoTo(room) => {
                self.wanted = Some(room.clone());
                self.goto = Some(room);
                self.take_goto().map_or(Taken::Nothing, Taken::Show)
            }
            Update::Timeline { room_id, items } => {
                if !shown(self, &room_id) || self.open_thread.is_some() {
                    return Taken::Nothing;
                }
                self.items = items;
                Taken::Items
            }
            Update::Thread {
                room_id,
                root,
                items,
            } => {
                if !shown(self, &room_id) || self.open_thread.as_deref() != Some(root.as_str()) {
                    return Taken::Nothing;
                }
                self.items = items;
                Taken::Items
            }
            Update::ThreadFailed { room_id, root } => {
                if !shown(self, &room_id) || self.open_thread.as_deref() != Some(root.as_str()) {
                    return Taken::Nothing;
                }
                self.leave_thread();
                Taken::LeftThread
            }
            Update::Typing { room_id, names } => {
                if !shown(self, &room_id) {
                    return Taken::Nothing;
                }
                self.typing = names;
                Taken::Typing
            }
            Update::Members { room_id, members } => {
                if !shown(self, &room_id) {
                    return Taken::Nothing;
                }
                self.members = members;
                Taken::Members
            }
            Update::LinkPreview {
                url,
                title,
                description,
            } => {
                self.link_previews.insert(url.clone(), (title, description));
                Taken::LinkPreview(url)
            }
            other => Taken::Other(other),
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
        // Each account has its own start: one signed in later brings its
        // rooms with what is unread in them, which is no news either.
        // (Those there from the first list on start with the program.)
        let first_seen = (!self.last_unread.is_empty()).then(Instant::now);
        for room in &rooms {
            if let Some(account) = self.accounts.get(room.account) {
                self.accounts_seen
                    .entry(account.profile.clone())
                    .or_insert(first_seen);
            }
        }
        let mut news = Vec::new();
        for room in &rooms {
            let settled = settled
                && self
                    .accounts
                    .get(room.account)
                    .and_then(|account| self.accounts_seen.get(&account.profile).copied())
                    .flatten()
                    .is_none_or(|seen| seen.elapsed() >= NOTIFICATION_WARMUP);
            let before = self
                .last_unread
                .insert(room.id.clone(), room.unread)
                .unwrap_or(0);
            let watching = self.focused && self.open_room.as_ref() == Some(&room.id);
            if settled
                && self.prefs.notifications
                && room.unread > before
                && !room.marked_unread
                && !watching
            {
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

    /// The rooms of `two_accounts`, and an invitation "!party".
    fn invited() -> ClientState {
        let mut state = two_accounts();
        let mut rooms = state.rooms.clone();
        rooms.push(RoomEntry {
            is_invite: true,
            ..room("!party", 0)
        });
        state.set_rooms(rooms);
        state
    }

    #[test]
    fn an_invitation_is_asked_about_first_and_joined_when_opened_again_soon() {
        let mut state = invited();
        state.open("!a");
        let first = state.open("!party");
        assert_eq!(
            first,
            Opening::Invitation {
                name: "!PARTY".into()
            }
        );
        assert!(first.commands().is_empty());
        assert_eq!(state.open_room.as_deref(), Some("!a"));
        assert!(state.invitation_asked().is_some());
        // Again: joined.
        let second = state.open("!party");
        assert!(matches!(&second.commands()[..], [Command::Open(room)] if room == "!party"));
        assert_eq!(state.invitation_asked(), None);

        // Another room opened in between, the front end saying the user
        // went on, or a long while: asked again.
        let mut state = invited();
        state.open("!party");
        state.open("!a");
        assert!(matches!(state.open("!party"), Opening::Invitation { .. }));
        state.forget_invitation();
        assert!(matches!(state.open("!party"), Opening::Invitation { .. }));
        state.invite_asked = Some(("!party".into(), Instant::now() - INVITATION_ASKED));
        assert!(matches!(state.open("!party"), Opening::Invitation { .. }));
        assert_eq!(state.open_room.as_deref(), Some("!a"));
    }

    #[test]
    fn a_room_a_command_asked_for_opens_without_asking_though_listed_as_an_invitation() {
        // "/join" of a room one was invited to: the list may still call
        // it an invitation when the account says to go there.
        let mut state = invited();
        assert!(matches!(
            state.take(Update::GoTo("!party".into())),
            Taken::Show(room) if room == "!party"
        ));
        assert!(matches!(state.open("!party"), Opening::Opened { .. }));
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

    #[test]
    fn an_account_added_later_brings_its_unread_rooms_without_news() {
        let mut state = two_accounts();
        // As at a start with one account only.
        state.accounts.truncate(1);
        state.accounts_seen.clear();
        state.last_unread.clear();
        let first: Vec<RoomEntry> = state
            .rooms
            .iter()
            .filter(|r| r.account == 0)
            .cloned()
            .collect();
        state.set_rooms(first.clone());
        state.started = Instant::now() - NOTIFICATION_WARMUP;
        // "work" signs in from the dialog: its rooms come with what is
        // unread in them.
        let work = two_accounts();
        state.accounts = work.accounts.clone();
        let mut all = first;
        all.extend(
            work.rooms
                .iter()
                .filter(|r| r.account == 1)
                .cloned()
                .map(|mut room| {
                    room.unread = 3;
                    room
                }),
        );
        assert!(state.set_rooms(all.clone()).is_empty());
        // The first account's rooms still tell.
        all[0].unread += 1;
        assert_eq!(state.set_rooms(all).len(), 1);
    }

    #[test]
    fn a_profile_name_used_again_in_one_run_brings_its_unread_rooms_without_news() {
        let mut state = two_accounts();
        let all = state.rooms.clone();
        state.set_rooms(all.clone());
        state.started = Instant::now() - NOTIFICATION_WARMUP;
        // "work" is signed out, and another account signs in under that
        // name: its rooms (the same room among them) come with what is
        // unread in them.
        let work = state.accounts[1].clone();
        state.forget_account(1);
        state.accounts.push(work);
        let unread: Vec<RoomEntry> = all
            .into_iter()
            .map(|mut room| {
                if room.account == 1 {
                    room.unread = 3;
                }
                room
            })
            .collect();
        assert!(state.set_rooms(unread).is_empty());
    }

    fn item(event_id: &str) -> Item {
        Item {
            event_id: event_id.into(),
            ..Item::default()
        }
    }

    fn timeline(room: &str, event_id: &str) -> Update {
        Update::Timeline {
            room_id: room.into(),
            items: vec![item(event_id)],
        }
    }

    fn thread(room: &str, root: &str, event_id: &str) -> Update {
        Update::Thread {
            room_id: room.into(),
            root: root.into(),
            items: vec![item(event_id)],
        }
    }

    fn shown(state: &ClientState) -> Vec<&str> {
        state
            .items
            .iter()
            .map(|item| item.event_id.as_str())
            .collect()
    }

    #[test]
    fn only_news_of_the_conversation_shown_is_taken() {
        let mut state = two_accounts();
        state.open("!a");
        assert!(matches!(
            state.take(timeline("!b", "$other")),
            Taken::Nothing
        ));
        assert!(matches!(state.take(timeline("!a", "$one")), Taken::Items));
        assert_eq!(shown(&state), ["$one"]);
        // A thread that is not shown says nothing.
        assert!(matches!(
            state.take(thread("!a", "$root", "$reply")),
            Taken::Nothing
        ));

        assert!(state.enter_thread("$root"));
        assert!(shown(&state).is_empty());
        // The room's own conversation waits while its thread is shown.
        assert!(matches!(state.take(timeline("!a", "$two")), Taken::Nothing));
        assert!(matches!(
            state.take(thread("!a", "$else", "$reply")),
            Taken::Nothing
        ));
        assert!(matches!(
            state.take(thread("!a", "$root", "$reply")),
            Taken::Items
        ));
        assert_eq!(shown(&state), ["$reply"]);

        let typing = |room: &str| Update::Typing {
            room_id: room.into(),
            names: vec!["Amy".into()],
        };
        assert!(matches!(state.take(typing("!b")), Taken::Nothing));
        assert!(matches!(state.take(typing("!a")), Taken::Typing));
        assert_eq!(state.typing, ["Amy"]);
        let members = |room: &str| Update::Members {
            room_id: room.into(),
            members: vec![Member::default()],
        };
        assert!(matches!(state.take(members("!b")), Taken::Nothing));
        assert!(matches!(state.take(members("!a")), Taken::Members));
        assert_eq!(state.members.len(), 1);
    }

    #[test]
    fn closing_leaves_nothing_of_the_room() {
        let mut state = two_accounts();
        assert!(!state.close());
        state.open("!a");
        state.enter_thread("$root");
        state.take(thread("!a", "$root", "$reply"));
        state.typing = vec!["Amy".into()];
        state.members = vec![Member::default()];
        assert!(state.close());
        assert_eq!((&state.open_room, &state.open_thread), (&None, &None));
        assert!(state.items.is_empty() && state.members.is_empty() && state.typing.is_empty());
    }

    #[test]
    fn a_thread_is_entered_from_the_room_or_another_thread() {
        let mut state = two_accounts();
        assert!(!state.enter_thread("$root"), "no room is open");
        state.open("!a");
        assert!(!state.enter_thread(""));
        assert!(state.enter_thread("$root"));
        assert!(!state.enter_thread("$root"), "it is shown already");
        assert!(state.enter_thread(crate::matrix::PINNED_VIEW));
        assert!(state.pinned_view());
    }

    #[test]
    fn a_thread_that_could_not_be_opened_leads_back_to_the_room() {
        let mut state = two_accounts();
        state.open("!a");
        state.enter_thread("$root");
        let failed = |room: &str, root: &str| Update::ThreadFailed {
            room_id: room.into(),
            root: root.into(),
        };
        // About a thread left meanwhile: nothing.
        assert!(matches!(state.take(failed("!a", "$else")), Taken::Nothing));
        assert!(matches!(state.take(failed("!b", "$root")), Taken::Nothing));
        assert!(matches!(
            state.take(failed("!a", "$root")),
            Taken::LeftThread
        ));
        assert_eq!(state.open_thread, None);
        // The room's conversation, which the account went back to, shows.
        assert!(matches!(state.take(timeline("!a", "$one")), Taken::Items));
    }

    #[test]
    fn a_room_to_go_to_is_shown_once_it_is_listed() {
        let mut state = two_accounts();
        assert!(matches!(
            state.take(Update::GoTo("!new".into())),
            Taken::Nothing
        ));
        let listed = Update::Rooms(vec![room("!a", 0), room("!new", 0)]);
        let Taken::Rooms { show, .. } = state.take(listed) else {
            panic!("rooms");
        };
        assert_eq!(show.as_deref(), Some("!new"));
        assert!(matches!(
            state.take(Update::GoTo("!a".into())),
            Taken::Show(room) if room == "!a"
        ));
        // What is not about rooms goes on to the front end.
        assert!(matches!(
            state.take(Update::Notice("hello".into())),
            Taken::Other(Update::Notice(_))
        ));
    }
}
