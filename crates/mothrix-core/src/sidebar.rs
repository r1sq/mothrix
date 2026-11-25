// SPDX-License-Identifier: MPL-2.0

//! Which rooms the room list shows, under which headings and in which
//! order: invites, favourites, the user's own sections, spaces, people,
//! rooms, low priority; folding, the account filter, the "only unread"
//! view and a typed filter. The terminal and the iPhone app list the same.
//!
//! How fold state is keyed (per account, when there are several) is
//! decided here alone: [`Folds`].

use std::collections::HashSet;

use crate::{
    accounts::Account,
    model::{RoomEntry, Section},
    prefs::{Prefs, SortMode},
    sections::{label, parent_name},
};

/// Which sections and accounts of the room list are folded, by fold key.
/// Kept per device. A section's key is its title, with `profile:` in
/// front when there are several accounts (each folds its own); an
/// account's rooms fold under `@profile`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folds(HashSet<String>);

impl Folds {
    /// The fold keys as the config file keeps them.
    pub fn new(keys: impl IntoIterator<Item = String>) -> Self {
        Self(keys.into_iter().collect())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.contains(key)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Fold what is kept under `key`, or unfold it.
    pub fn toggle(&mut self, key: &str) {
        if !self.0.remove(key) {
            self.0.insert(key.to_owned());
        }
    }

    /// Unfold everything.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// The keys, sorted, for the config file.
    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.0.iter().cloned().collect();
        keys.sort();
        keys
    }

    fn section_key(profile: Option<&str>, title: &str) -> String {
        match profile {
            Some(profile) => format!("{profile}:{title}"),
            None => title.to_owned(),
        }
    }

    fn account_key(profile: &str) -> String {
        format!("@{profile}")
    }

    /// Every key a section can be folded under: alone, and under each
    /// account.
    fn section_keys(title: &str, accounts: &[Account]) -> Vec<String> {
        std::iter::once(None)
            .chain(
                accounts
                    .iter()
                    .map(|account| Some(account.profile.as_str())),
            )
            .map(|profile| Self::section_key(profile, title))
            .collect()
    }

    /// A section got a new name: where it was folded, it stays folded.
    pub(crate) fn rename_section(&mut self, from: &str, to: &str, accounts: &[Account]) {
        let renamed = Self::section_keys(to, accounts);
        for (old, new) in Self::section_keys(from, accounts).iter().zip(renamed) {
            if self.0.remove(old) {
                self.0.insert(new);
            }
        }
    }

    /// A section is gone: so is its fold state.
    pub(crate) fn forget_section(&mut self, name: &str, accounts: &[Account]) {
        for key in Self::section_keys(name, accounts) {
            self.0.remove(&key);
        }
    }

    /// An account is gone (signed out), or its profile's name goes to
    /// another one: what was folded for it is not folded for the next
    /// account of that name. Returns whether anything was.
    pub fn forget_profile(&mut self, profile: &str) -> bool {
        let before = self.0.len();
        let sections = Self::section_key(Some(profile), "");
        let account = Self::account_key(profile);
        self.0
            .retain(|key| *key != account && !key.starts_with(&sections));
        self.0.len() != before
    }
}

/// One line of the room list.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Row {
    Header {
        title: String,
    },
    /// A folded section: its header with what is inside.
    Folded {
        title: String,
        rooms: usize,
        unread: usize,
        mentions: usize,
    },
    /// Index into the rooms.
    Room {
        index: usize,
    },
    /// The line above the rooms of an account (index into the accounts),
    /// when several are shown together.
    Account {
        index: usize,
    },
}

/// What a click on a line does.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", content = "key", rename_all = "snake_case")]
pub enum Target {
    /// Fold or unfold what is kept under this fold key.
    Fold(String),
    /// Open the room with this key.
    Room(String),
}

/// One line of the room list, ready to draw.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Line {
    pub row: Row,
    /// The section, account or room name (a subsection without its
    /// parent's name).
    pub label: String,
    /// A subsection, or a room in one.
    pub indent: bool,
    /// Unread messages and mentions in it.
    pub unread: usize,
    pub mentions: usize,
    /// A folded section or account.
    pub folded: bool,
    pub target: Target,
}

/// What the room list is made from.
pub struct Sidebar<'a> {
    pub rooms: &'a [RoomEntry],
    pub accounts: &'a [Account],
    /// The user's own sections, shown in the mixed list.
    pub sections: &'a [Section],
    pub prefs: &'a Prefs,
    pub folded: &'a Folds,
    /// What is typed into the filter.
    pub filter: &'a str,
    /// Only chats with something unread.
    pub unread_only: bool,
    /// The account shown alone, if any.
    pub account_filter: Option<usize>,
    /// The open room stays in the unread view once it has been read.
    pub open_room: Option<&'a str>,
}

impl Sidebar<'_> {
    /// The room list as it is drawn, line by line.
    pub fn lines(&self) -> Vec<Line> {
        // Section headers fold for the account they are under.
        let mut account = self.account_filter.filter(|_| self.accounts.len() > 1);
        self.rows()
            .into_iter()
            .map(|row| {
                if let Row::Account { index } = row {
                    account = Some(index);
                }
                self.line(row, account)
            })
            .collect()
    }

    fn line(&self, row: Row, account: Option<usize>) -> Line {
        let section = |title: &str, unread: usize, mentions: usize, folded: bool| Line {
            row: row.clone(),
            label: label(title).to_owned(),
            indent: parent_name(title).is_some(),
            unread,
            mentions,
            folded,
            target: Target::Fold(self.fold_key(account, title)),
        };
        match &row {
            Row::Header { title } => section(title, 0, 0, false),
            Row::Folded {
                title,
                unread,
                mentions,
                ..
            } => section(title, *unread, *mentions, true),
            Row::Account { index } => {
                let key = self.account_fold_key(*index);
                let (unread, mentions) = self.account_unread(*index);
                Line {
                    label: self.accounts[*index].label.clone(),
                    indent: false,
                    unread,
                    mentions,
                    folded: self.folded.contains(&key),
                    target: Target::Fold(key),
                    row,
                }
            }
            Row::Room { index } => {
                let room = &self.rooms[*index];
                // The unread view mixes the accounts: say whose it is.
                let label = if self.unread_only && self.accounts.len() > 1 {
                    let account = &self.accounts[room.account.min(self.accounts.len() - 1)];
                    format!("{} · {}", room.name, account.label)
                } else {
                    room.name.clone()
                };
                Line {
                    label,
                    indent: !self.unread_only && self.section_of(room).contains('/'),
                    unread: room.unread as usize,
                    mentions: room.mentions as usize,
                    folded: false,
                    target: Target::Room(room.id.clone()),
                    row,
                }
            }
        }
    }

    /// Unread messages and mentions in all rooms of an account.
    pub fn account_unread(&self, account: usize) -> (usize, usize) {
        self.rooms
            .iter()
            .filter(|room| room.account == account)
            .fold((0, 0), |(unread, mentions), room| {
                (
                    unread + room.unread as usize,
                    mentions + room.mentions as usize,
                )
            })
    }

    /// The lines in display order, each section under a header, narrowed
    /// by the filter when one is typed. Folded sections show only their
    /// header, except while filtering; a folded section hides its
    /// subsections with it. With several accounts, each one's rooms come
    /// under a line of their own, unless only one account is shown.
    pub fn rows(&self) -> Vec<Row> {
        let needle = self.filter.trim().to_lowercase();
        // Only what is unread: one list across all accounts and sections,
        // mentions first. The open room stays, so that it does not vanish
        // from under the cursor once it has been read.
        if self.unread_only {
            let mut unread: Vec<usize> = self
                .rooms
                .iter()
                .enumerate()
                .filter(|(_, room)| !room.is_space || room.is_invite)
                .filter(|(_, room)| {
                    room.unread > 0
                        || room.mentions > 0
                        || room.is_invite
                        || self.open_room == Some(room.id.as_str())
                })
                .filter(|(_, room)| needle.is_empty() || room.name.to_lowercase().contains(&needle))
                .map(|(index, _)| index)
                .collect();
            unread.sort_by_key(|index| self.rooms[*index].mentions == 0);
            if unread.is_empty() {
                return Vec::new();
            }
            let mut rows = vec![Row::Header {
                title: "UNREAD".to_owned(),
            }];
            rows.extend(unread.into_iter().map(|index| Row::Room { index }));
            return rows;
        }
        let listed = |account: Option<usize>| -> Vec<(usize, String)> {
            self.rooms
                .iter()
                .enumerate()
                .filter(|(_, room)| !room.is_space || room.is_invite)
                .filter(|(_, room)| account.is_none_or(|account| room.account == account))
                .filter(|(_, room)| needle.is_empty() || room.name.to_lowercase().contains(&needle))
                .map(|(index, room)| (index, self.section_of(room)))
                .collect()
        };
        if self.accounts.len() < 2 {
            return self.section_rows(&listed(None), None, &needle);
        }
        if let Some(account) = self.account_filter {
            return self.section_rows(&listed(Some(account)), Some(account), &needle);
        }
        let mut rows = Vec::new();
        for account in 0..self.accounts.len() {
            let listed = listed(Some(account));
            // While filtering, accounts without a match stay out of the way.
            if listed.is_empty() && !needle.is_empty() {
                continue;
            }
            rows.push(Row::Account { index: account });
            if needle.is_empty() && self.folded.contains(&self.account_fold_key(account)) {
                continue;
            }
            rows.extend(self.section_rows(&listed, Some(account), &needle));
        }
        rows
    }

    /// The title of the section a room is listed in. Own sections (mixed
    /// list) come before the space a room is in.
    pub fn section_of(&self, room: &RoomEntry) -> String {
        let own = || {
            self.sections
                .iter()
                .find(|section| section.rooms.contains(&room.id))
                .map(|section| section.name.clone())
        };
        let space = || (self.prefs.spaces && !room.space.is_empty()).then(|| room.space.clone());
        if room.is_invite {
            "INVITES".to_owned()
        } else if room.favourite {
            "PINNED".to_owned()
        } else if room.low_priority {
            "LOW PRIORITY".to_owned()
        } else if self.prefs.mixed_list {
            own().or_else(space).unwrap_or_else(|| "CHATS".to_owned())
        } else if let Some(space) = space() {
            space
        } else if room.is_dm {
            "PEOPLE".to_owned()
        } else {
            "ROOMS".to_owned()
        }
    }

    /// The name the fold state of a section is kept under. With several
    /// accounts each has its own, so that folding "CHATS" of one leaves
    /// the others open.
    pub fn fold_key(&self, account: Option<usize>, title: &str) -> String {
        let profile = account
            .and_then(|account| self.accounts.get(account))
            .filter(|_| self.accounts.len() > 1)
            .map(|account| account.profile.as_str());
        Folds::section_key(profile, title)
    }

    /// The name the fold state of an account's rooms is kept under.
    pub fn account_fold_key(&self, account: usize) -> String {
        Folds::account_key(&self.accounts[account].profile)
    }

    /// The sections in display order. The user's own sections (kept with
    /// subsections right after their parent) belong to the mixed list;
    /// spaces follow by name, each before its subspaces.
    fn section_order(&self) -> Vec<String> {
        let mut order = vec!["INVITES".to_owned(), "PINNED".to_owned()];
        if self.prefs.mixed_list {
            order.extend(self.sections.iter().map(|section| section.name.clone()));
        }
        if self.prefs.spaces {
            let mut spaces: Vec<String> = Vec::new();
            for room in self.rooms {
                if room.space.is_empty() {
                    continue;
                }
                // A subspace needs the line of its space above it.
                let parent = parent_name(&room.space).map(str::to_owned);
                for name in parent.into_iter().chain([room.space.clone()]) {
                    if !spaces.contains(&name) && !order.contains(&name) {
                        spaces.push(name);
                    }
                }
            }
            spaces.sort_by_key(|name| name.to_lowercase());
            order.extend(spaces);
        }
        if self.prefs.mixed_list {
            order.push("CHATS".to_owned());
        } else {
            order.extend(["ROOMS".to_owned(), "PEOPLE".to_owned()]);
        }
        order.push("LOW PRIORITY".to_owned());
        // Each once: an own section or a space can be called what a
        // built-in section is called, and would else be listed twice with
        // all its rooms.
        let mut once: Vec<String> = Vec::with_capacity(order.len());
        for title in order {
            if !once.contains(&title) {
                once.push(title);
            }
        }
        once
    }

    fn section_rows(
        &self,
        listed: &[(usize, String)],
        account: Option<usize>,
        needle: &str,
    ) -> Vec<Row> {
        let folded =
            |title: &str| needle.is_empty() && self.folded.contains(&self.fold_key(account, title));

        let mut rows = Vec::new();
        for title in self.section_order() {
            if parent_name(&title).is_some_and(folded) {
                continue;
            }
            let mut members: Vec<usize> = listed
                .iter()
                .filter(|(_, section)| *section == title)
                .map(|(index, _)| *index)
                .collect();
            // Chats in its subsections.
            let below: Vec<usize> = listed
                .iter()
                .filter(|(_, section)| parent_name(section) == Some(title.as_str()))
                .map(|(index, _)| *index)
                .collect();
            // Stable sorts: ties keep the server's most-recent-first order.
            match self.prefs.sort {
                SortMode::Recent => {}
                SortMode::Name => {
                    members.sort_by_key(|index| self.rooms[*index].name.to_lowercase());
                }
                SortMode::Unread => members.sort_by_key(|index| {
                    let room = &self.rooms[*index];
                    (room.mentions == 0, room.unread == 0)
                }),
            }
            if members.is_empty() && below.is_empty() {
                continue;
            }
            if folded(&title) {
                let inside = || {
                    members
                        .iter()
                        .chain(&below)
                        .map(|index| &self.rooms[*index])
                };
                rows.push(Row::Folded {
                    rooms: inside().count(),
                    unread: inside().map(|room| room.unread as usize).sum(),
                    mentions: inside().map(|room| room.mentions as usize).sum(),
                    title,
                });
                continue;
            }
            rows.push(Row::Header { title });
            rows.extend(members.into_iter().map(|index| Row::Room { index }));
        }
        rows
    }
}
