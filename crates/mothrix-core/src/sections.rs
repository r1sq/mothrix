// SPDX-License-Identifier: MPL-2.0

//! The user's own sections, kept in each account's account data
//! ([`EVENT_TYPE`]) so that every device of the account lists rooms the
//! same way: the terminal, the phone, a second computer.
//!
//! A UI holds one list of sections over all its accounts, with room keys
//! (`profile/!id`). Each account stores its *share* of it: every section
//! (in order) that holds rooms of that account or no rooms at all, with
//! only that account's rooms, as plain room ids. [`Sync`] keeps the two in
//! step: what an account says is merged into the list ([`Sync::heard`]),
//! and an account whose share changed here is told ([`Sync::to_send`]).
//! What stays on the device: which sections are folded.
//!
//! The backend of an account reads what it stores and tells of it
//! ([`Watch`]). Every write says which device made it and as which of its
//! writes ([`Written`]): that is how a device knows an older write of its
//! own coming back from one another device made with the same content.
//!
//! The content is not encrypted: the homeserver can read section names.

use std::ops::Deref;

use serde::{Deserialize, Serialize};

use crate::{
    accounts::{Account, split},
    model::Section,
    sidebar::Folds,
};

/// The account data type mothrix keeps sections under.
pub const EVENT_TYPE: &str = "de.tmrk.mothrix.sections";

/// The account data's content. Without `sections` it is not ours to
/// read (`{}`, a later form): taking that for "no sections" would have
/// this device write its own over it.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub sections: Vec<Section>,
    /// Who wrote it; none from a mothrix that did not say yet (0.3.0 and
    /// before, which pass over this when they read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Written>,
}

/// The device that wrote the stored sections, and which of its writes it
/// was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Written {
    /// The device's id in its account.
    pub device: String,
    /// A number that grows with every write of that device, also over
    /// its restarts ([`next_write`]).
    pub write: u64,
}

/// The number of a device's next write, after `last` (0: none yet in this
/// run): the time in milliseconds, so that it is larger than those of
/// earlier runs too, and always larger than the last.
pub fn next_write(last: u64, now_ms: u64) -> u64 {
    now_ms.max(last + 1)
}

/// What an account says of the sections it stores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    /// It stores none yet.
    Nothing,
    Sections(Vec<Section>),
    /// What it stores cannot be read here (a newer mothrix's form): not
    /// "nothing", and nothing is to be written over it.
    Unreadable,
}

impl Said {
    /// What stored content (JSON, if any) says.
    fn of(content: Option<&str>) -> Self {
        match content.map(serde_json::from_str::<Stored>) {
            None => Self::Nothing,
            Some(Ok(stored)) => Self::Sections(stored.sections),
            Some(Err(_)) => Self::Unreadable,
        }
    }
}

/// What the backend of one account keeps to tell of the sections the
/// account stores: at the start what the server has, then what each sync
/// brings into the local store, which it looks at every few seconds.
/// Plain data in and out; `matrix::watch_sections` does the reading.
#[derive(Debug)]
pub struct Watch {
    /// This device's id, as its writes name it.
    device: String,
    /// The local store's content when last looked at.
    store: Option<String>,
    /// What was last told; the outer `None`: nothing yet.
    told: Option<Option<String>>,
}

impl Watch {
    /// For this device, with what the local store holds at the start
    /// (read before the server is asked).
    pub fn new(device: &str, store: Option<String>) -> Self {
        Self {
            device: device.to_owned(),
            store,
            told: None,
        }
    }

    /// Whether the account has said what it stores.
    pub fn has_told(&self) -> bool {
        self.told.is_some()
    }

    fn tell(&mut self, content: Option<String>) -> Said {
        let said = Said::of(content.as_deref());
        self.told = Some(content);
        said
    }

    /// The server answered what the account stores: that is said.
    pub fn fetched(&mut self, content: Option<String>) -> Said {
        self.tell(content)
    }

    /// The server could not be asked. What the local store holds is said
    /// in its place; with nothing there, nothing is said (and the server
    /// is to be asked again): an empty store does not mean that the
    /// account stores none, and saying so would send this device's
    /// sections up over those the server has.
    pub fn not_fetched(&mut self) -> Option<Said> {
        let store = self.store.clone()?;
        Some(self.tell(Some(store)))
    }

    /// The local store holds `now`; `latest` is the number of this
    /// device's newest write asked for (0: none in this run). Returns
    /// what there is to say, if anything:
    ///
    /// - not what the store held before (at the start that may be an
    ///   older copy than the server's answer, and is no news),
    /// - not what was said last (the server's answer reaching the store),
    /// - not an older write of this device coming back after a newer one
    ///   was sent: the newer one stands.
    pub fn look(&mut self, now: Option<String>, latest: u64) -> Option<Said> {
        // The store does not lose what it had; a failed reading looks so.
        let now = now?;
        if self.store.as_ref() == Some(&now) {
            return None;
        }
        self.store = Some(now.clone());
        if self.told.as_ref() == Some(&Some(now.clone())) {
            return None;
        }
        let stale = serde_json::from_str::<Stored>(&now)
            .ok()
            .and_then(|stored| stored.by)
            .is_some_and(|by| by.device == self.device && by.write < latest);
        if stale {
            return None;
        }
        Some(self.tell(Some(now)))
    }
}

/// Titles of the room list's built-in sections; own sections cannot take
/// them.
pub const BUILT_IN: [&str; 6] = [
    "INVITES",
    "PINNED",
    "CHATS",
    "ROOMS",
    "PEOPLE",
    "LOW PRIORITY",
];

/// The section a subsection (`Parent/Name`) belongs to. Spaces name their
/// subspaces the same way.
pub fn parent_name(name: &str) -> Option<&str> {
    name.split_once('/').map(|(parent, _)| parent)
}

/// What a section is called under its parent: `Name` of `Parent/Name`.
pub fn label(name: &str) -> &str {
    name.split_once('/').map_or(name, |(_, label)| label)
}

/// The user's own sections over all accounts (rooms by their keys), in
/// display order and always tidy: a subsection has its parent and comes
/// right after it, and a section is there only while it holds a room or a
/// subsection that does. Every change goes through here, so that both
/// front ends and the sync keep the same rules. Names match regardless of
/// case.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnSections(Vec<Section>);

impl Deref for OwnSections {
    type Target = [Section];

    fn deref(&self) -> &[Section] {
        &self.0
    }
}

impl OwnSections {
    /// These sections, tidied.
    pub fn new(sections: Vec<Section>) -> Self {
        // One section of a name (a config written by hand can have two):
        // the later one's rooms join the first.
        let mut once: Vec<Section> = Vec::with_capacity(sections.len());
        for section in sections {
            match once.iter_mut().find(|known| known.name == section.name) {
                Some(known) => known.rooms.extend(section.rooms),
                None => once.push(section),
            }
        }
        let mut own = Self(once);
        own.tidy();
        own
    }

    /// Put a room into the section `name` (made if new; `Parent/Name`
    /// makes a subsection), or into none. The error says what is wrong
    /// with the name; then nothing changed.
    pub fn assign(&mut self, room: &str, name: Option<&str>) -> Result<(), String> {
        let name = name
            .filter(|name| !name.trim().is_empty())
            .map(|typed| self.clean_name(typed, None).map(|(name, _)| name))
            .transpose()?;
        for section in &mut self.0 {
            section.rooms.retain(|key| key != room);
        }
        if let Some(name) = name {
            match self.0.iter_mut().find(|section| section.name == name) {
                Some(section) => section.rooms.push(room.to_owned()),
                None => self.0.push(Section {
                    name,
                    rooms: vec![room.to_owned()],
                }),
            }
        }
        self.tidy();
        Ok(())
    }

    /// Give the section `from` a new name, and its subsections with it;
    /// `Parent/Name` makes it a subsection (or moves it to another
    /// parent). What was folded stays folded. Returns the new name.
    pub fn rename(
        &mut self,
        from: &str,
        typed: &str,
        folds: &mut Folds,
        accounts: &[Account],
    ) -> Result<String, String> {
        let index = self
            .0
            .iter()
            .position(|section| section.name == from)
            .ok_or_else(|| format!("There is no section “{from}”."))?;
        let (name, taken) = self.clean_name(typed, Some(index))?;
        if taken {
            return Err(format!("“{name}” is taken."));
        }
        let prefix = format!("{from}/");
        if name.contains('/')
            && self
                .0
                .iter()
                .any(|section| section.name.starts_with(&prefix))
        {
            return Err("It has subsections, so it cannot become one.".to_owned());
        }
        for section in &mut self.0 {
            let renamed = if section.name == from {
                Some(name.clone())
            } else {
                section
                    .name
                    .strip_prefix(&prefix)
                    .map(|rest| format!("{name}/{rest}"))
            };
            if let Some(renamed) = renamed {
                folds.rename_section(&section.name, &renamed, accounts);
                section.name = renamed;
            }
        }
        self.tidy();
        Ok(name)
    }

    /// Take the section `name` away, with its subsections; their rooms go
    /// back to the general list.
    pub fn remove(&mut self, name: &str, folds: &mut Folds, accounts: &[Account]) {
        let prefix = format!("{name}/");
        self.0.retain(|section| {
            let goes = section.name == name || section.name.starts_with(&prefix);
            if goes {
                folds.forget_section(&section.name, accounts);
            }
            !goes
        });
        self.tidy();
    }

    /// Swap the section `name` with its neighbour of the same level, up or
    /// down; subsections travel with their parent. Returns whether it
    /// moved.
    pub fn move_section(&mut self, name: &str, down: bool) -> bool {
        let Some(index) = self.0.iter().position(|section| section.name == name) else {
            return false;
        };
        let parent = parent_name(name);
        let siblings: Vec<usize> = (0..self.0.len())
            .filter(|at| parent_name(&self.0[*at].name) == parent)
            .collect();
        let Some(at) = siblings.iter().position(|sibling| *sibling == index) else {
            return false;
        };
        let other = if down {
            Some(at + 1)
        } else {
            at.checked_sub(1)
        };
        let Some(&other) = other.and_then(|other| siblings.get(other)) else {
            return false;
        };
        self.0.swap(index, other);
        self.tidy();
        true
    }

    /// Take out the rooms of the account at `account` of `prefixes` (it
    /// is signed out, or another account takes its profile's name).
    /// Returns whether there were any.
    pub fn forget_account(&mut self, prefixes: &[String], account: usize) -> bool {
        let before = self.0.clone();
        for section in &mut self.0 {
            section
                .rooms
                .retain(|key| split(prefixes, key).is_none_or(|(owner, _)| owner != account));
        }
        self.tidy();
        self.0 != before
    }

    /// Tidy a typed section name ("work / Clients" becomes "Work/Clients"
    /// if "Work" exists). Returns the name and whether a section other
    /// than the one at `except` has it already; the error says what is
    /// wrong with it.
    fn clean_name(&self, typed: &str, except: Option<usize>) -> Result<(String, bool), String> {
        let same = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();
        let find = |name: &str| {
            self.0
                .iter()
                .enumerate()
                .find(|(index, section)| Some(*index) != except && same(&section.name, name))
                .map(|(_, section)| section.name.clone())
        };
        // A section that exists is taken as it is.
        if let Some(existing) = find(typed.trim()) {
            return Ok((existing, true));
        }
        let parts: Vec<&str> = typed.split('/').map(str::trim).collect();
        if parts.len() > 2 {
            return Err("Sections go one level deep: Parent/Name.".to_owned());
        }
        if parts.iter().any(|part| part.is_empty()) {
            return Err("A section needs a name.".to_owned());
        }
        if BUILT_IN.contains(&parts[0].to_uppercase().as_str()) {
            return Err(format!("“{}” is taken; pick another name.", parts[0]));
        }
        let name = match parts[..] {
            [parent, sub] => {
                let parent = self
                    .0
                    .iter()
                    .find(|section| same(&section.name, parent))
                    .map_or(parent, |section| section.name.as_str());
                format!("{parent}/{sub}")
            }
            _ => parts[0].to_owned(),
        };
        Ok(match find(&name) {
            Some(existing) => (existing, true),
            None => (name, false),
        })
    }

    /// A subsection has its parent and comes right after it (in the order
    /// subsections had), and a section exists only while it holds a room
    /// or a subsection that does.
    fn tidy(&mut self) {
        let mut sections = std::mem::take(&mut self.0);
        sections
            .retain(|section| parent_name(&section.name).is_none() || !section.rooms.is_empty());
        let parents: Vec<String> = sections
            .iter()
            .filter_map(|section| parent_name(&section.name).map(str::to_owned))
            .collect();
        for parent in &parents {
            if !sections.iter().any(|section| section.name == *parent) {
                // Where its first subsection stands.
                let at = sections
                    .iter()
                    .position(|section| parent_name(&section.name) == Some(parent.as_str()))
                    .unwrap_or(sections.len());
                let section = Section {
                    name: parent.clone(),
                    rooms: Vec::new(),
                };
                sections.insert(at, section);
            }
        }
        sections.retain(|section| !section.rooms.is_empty() || parents.contains(&section.name));
        let (tops, subs): (Vec<_>, Vec<_>) = sections
            .into_iter()
            .partition(|section| parent_name(&section.name).is_none());
        for top in tops {
            let name = top.name.clone();
            self.0.push(top);
            self.0.extend(
                subs.iter()
                    .filter(|sub| parent_name(&sub.name) == Some(name.as_str()))
                    .cloned(),
            );
        }
    }
}

/// The running account a room key belongs to, and the room's id there;
/// none for a room of an account that is not running (signed out, left
/// out at this start): its keys wait in the sections, and are neither
/// another account's to store on its server nor to take out.
fn owner<'a>(prefixes: &[String], key: &'a str) -> Option<(usize, &'a str)> {
    split(prefixes, key)
}

/// What of `sections` belongs to `account`: every section that holds its
/// rooms, and the parent of each such subsection, in order, with only its
/// rooms (as ids). Nothing else: a section that would hold nothing on a
/// device with only this account would be tidied away there and sent back
/// without, for this device to add again, for ever.
pub fn share(sections: &[Section], prefixes: &[String], account: usize) -> Vec<Section> {
    let own: Vec<Section> = sections
        .iter()
        .map(|section| Section {
            name: section.name.clone(),
            rooms: section
                .rooms
                .iter()
                .filter_map(|key| {
                    let (owner, id) = owner(prefixes, key)?;
                    (owner == account).then(|| id.to_owned())
                })
                .collect(),
        })
        .collect();
    let parents: Vec<&str> = own
        .iter()
        .filter(|section| !section.rooms.is_empty())
        .filter_map(|section| parent_name(&section.name))
        .collect();
    own.iter()
        .filter(|section| !section.rooms.is_empty() || parents.contains(&section.name.as_str()))
        .cloned()
        .collect()
}

/// Put `account`'s share into `sections`: its rooms go where the share
/// says, sections it names appear (after the one before them) and stand
/// in the share's order among themselves, and a section that held its
/// rooms and that it no longer names goes when nothing else is left in
/// it.
fn merge(sections: &mut Vec<Section>, prefixes: &[String], account: usize, theirs: &[Section]) {
    let prefix = prefixes.get(account).cloned().unwrap_or_default();
    let mine = |key: &String| owner(prefixes, key).is_some_and(|(owner, _)| owner == account);
    let had: Vec<String> = sections
        .iter()
        .filter(|section| section.rooms.iter().any(mine))
        .map(|section| section.name.clone())
        .collect();
    for section in sections.iter_mut() {
        section.rooms.retain(|key| !mine(key));
    }
    let mut after: Option<usize> = None;
    for incoming in theirs {
        let keys = incoming.rooms.iter().map(|id| format!("{prefix}{id}"));
        let at = match sections.iter().position(|s| s.name == incoming.name) {
            Some(at) => at,
            None => {
                let at = after.map_or(sections.len(), |before| before + 1);
                sections.insert(
                    at,
                    Section {
                        name: incoming.name.clone(),
                        rooms: Vec::new(),
                    },
                );
                at
            }
        };
        sections[at].rooms.extend(keys);
        after = Some(at);
    }
    // Their order, in the places these sections have here: a section moved
    // on another device moves here too. Keeping this device's order would
    // have each of the two write its own back whenever it hears the other.
    let places: Vec<usize> = (0..sections.len())
        .filter(|&at| theirs.iter().any(|named| named.name == sections[at].name))
        .collect();
    let mut ordered: Vec<Section> = Vec::new();
    for incoming in theirs {
        if !ordered.iter().any(|section| section.name == incoming.name)
            && let Some(section) = sections.iter().find(|s| s.name == incoming.name)
        {
            ordered.push(section.clone());
        }
    }
    if places.len() == ordered.len() {
        for (at, section) in places.into_iter().zip(ordered) {
            sections[at] = section;
        }
    }
    sections.retain(|section| {
        !section.rooms.is_empty()
            || !had.contains(&section.name)
            || theirs.iter().any(|named| named.name == section.name)
    });
}

/// What one account was last known to store, and whether it has spoken
/// yet. Nothing is sent to it before it has: the server may hold sections
/// this device has not seen.
#[derive(Debug, Default)]
pub struct Sync {
    /// `None` until heard; then what it stores (`None` inside: nothing
    /// yet).
    known: Option<Option<Vec<Section>>>,
}

impl Sync {
    /// The account (at `account` in `prefixes`) says what it stores.
    /// Returns whether `sections` changed.
    pub fn said(
        &mut self,
        sections: &mut OwnSections,
        prefixes: &[String],
        account: usize,
        said: Said,
    ) -> bool {
        match said {
            Said::Nothing => self.heard(sections, prefixes, account, None),
            Said::Sections(stored) => self.heard(sections, prefixes, account, Some(stored)),
            // As before it spoke: nothing is sent to it, until it stores
            // something this device can read again.
            Said::Unreadable => {
                self.known = None;
                false
            }
        }
    }

    /// The account (at `account` in `prefixes`) stores this (`None`:
    /// nothing yet). Returns whether `sections` changed. What it
    /// stores is tidied on the way in; when that changed it,
    /// [`Sync::to_send`] writes the tidy form back.
    pub fn heard(
        &mut self,
        sections: &mut OwnSections,
        prefixes: &[String],
        account: usize,
        stored: Option<Vec<Section>>,
    ) -> bool {
        // Only room ids: what an account stores is its server's word, and
        // anything else could name another account's room here.
        let stored = stored.map(|theirs| {
            theirs
                .into_iter()
                .map(|mut section| {
                    section.rooms.retain(|id| id.starts_with('!'));
                    section
                })
                .collect::<Vec<_>>()
        });
        let changed = match &stored {
            Some(theirs) if *theirs != share(sections, prefixes, account) => {
                let before = sections.0.clone();
                merge(&mut sections.0, prefixes, account, theirs);
                sections.tidy();
                sections.0 != before
            }
            _ => false,
        };
        self.known = Some(stored);
        changed
    }

    /// The account's share of `sections`, if it differs from what it
    /// stores: to be sent. Counted as stored from now on. An account
    /// storing nothing is sent nothing until it has a share; one that has
    /// not spoken yet, nothing at all.
    pub fn to_send(
        &mut self,
        sections: &[Section],
        prefixes: &[String],
        account: usize,
    ) -> Option<Vec<Section>> {
        let stored = self.known.as_mut()?;
        let now = share(sections, prefixes, account);
        let same = match stored {
            Some(stored) => *stored == now,
            None => now.is_empty(),
        };
        if same {
            return None;
        }
        *stored = Some(now.clone());
        Some(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(name: &str, rooms: &[&str]) -> Section {
        Section {
            name: name.into(),
            rooms: rooms.iter().map(|room| (*room).into()).collect(),
        }
    }

    /// Account 0 is "default" (no prefix), account 1 is "work/".
    fn prefixes() -> Vec<String> {
        vec![String::new(), "work/".into()]
    }

    #[test]
    fn an_account_s_share_holds_its_rooms_and_empty_sections() {
        let sections = vec![
            section("Team", &["!a", "work/!b"]),
            section("Family", &["!c"]),
            section("Work", &[]),
            section("Work/Clients", &["work/!d"]),
        ];
        assert_eq!(
            share(&sections, &prefixes(), 0),
            [section("Team", &["!a"]), section("Family", &["!c"])]
        );
        // A subsection comes with its parent; the other account, with no
        // room under it, does not name the parent.
        assert_eq!(
            share(&sections, &prefixes(), 1),
            [
                section("Team", &["!b"]),
                section("Work", &[]),
                section("Work/Clients", &["!d"])
            ]
        );
    }

    /// One device of the account in a [`World`]: what its UI holds, what
    /// `Accounts` keeps of the account, and its backend's reader.
    struct Device {
        sections: OwnSections,
        sync: Sync,
        watch: Watch,
        prefixes: Vec<String>,
        /// The number of its newest write.
        latest: u64,
        /// How many of the server's writes it has looked at.
        seen: usize,
    }

    impl Device {
        fn take(&mut self, said: Said) {
            self.sync.said(&mut self.sections, &self.prefixes, 0, said);
        }
    }

    /// Devices of one account and what it stores, as the content the
    /// server took, write after write. A device reads the server's answer
    /// at its start, then what its store holds when it looks, and sends
    /// what it then has to say.
    struct World {
        devices: Vec<Device>,
        stored: Vec<String>,
    }

    /// The content a device writes, or (without one) an older mothrix.
    fn content(sections: &[Section], by: Option<(&str, u64)>) -> String {
        serde_json::to_string(&Stored {
            sections: sections.to_vec(),
            by: by.map(|(device, write)| Written {
                device: device.into(),
                write,
            }),
        })
        .unwrap()
    }

    impl World {
        fn new(stored: Vec<Section>, devices: Vec<(OwnSections, Vec<String>)>) -> Self {
            let first = content(&stored, None);
            let devices = devices
                .into_iter()
                .enumerate()
                .map(|(index, (sections, prefixes))| {
                    let mut device = Device {
                        sections,
                        sync: Sync::default(),
                        watch: Watch::new(&format!("DEVICE{index}"), Some(first.clone())),
                        prefixes,
                        latest: 0,
                        seen: 1,
                    };
                    let said = device.watch.fetched(Some(first.clone()));
                    device.take(said);
                    device
                })
                .collect();
            World {
                devices,
                stored: vec![first],
            }
        }

        /// The device sends its share, if it has something to say.
        fn send(&mut self, index: usize) -> bool {
            let device = &mut self.devices[index];
            let Some(share) = device.sync.to_send(&device.sections, &device.prefixes, 0) else {
                return false;
            };
            device.latest = next_write(device.latest, 0);
            let by = format!("DEVICE{index}");
            self.stored
                .push(content(&share, Some((&by, device.latest))));
            true
        }

        /// The device looks at its store, which holds the newest write:
        /// what was written between two looks it never sees.
        fn look(&mut self, index: usize) {
            let device = &mut self.devices[index];
            device.seen = self.stored.len();
            let newest = self.stored.last().cloned();
            if let Some(said) = device.watch.look(newest, device.latest) {
                device.take(said);
            }
        }

        /// Run until all are silent, each device seeing every write (its
        /// own too) in the order the server took them; the number of
        /// writes that took.
        fn settle(&mut self) -> usize {
            let before = self.stored.len();
            for _ in 0..20 {
                let mut quiet = true;
                for index in 0..self.devices.len() {
                    while self.devices[index].seen < self.stored.len() {
                        let device = &mut self.devices[index];
                        let stored = self.stored[device.seen].clone();
                        device.seen += 1;
                        if let Some(said) = device.watch.look(Some(stored), device.latest) {
                            device.take(said);
                        }
                    }
                    if self.send(index) {
                        quiet = false;
                    }
                }
                if quiet {
                    return self.stored.len() - before;
                }
            }
            panic!("the devices never stop writing: {:#?}", self.stored);
        }
    }

    #[test]
    fn a_change_undone_at_once_does_not_swallow_what_another_device_writes_later() {
        let one = vec![String::new()];
        let before = vec![section("Team", &["!a"])];
        let device = || (OwnSections::new(before.clone()), one.clone());
        let mut world = World::new(before.clone(), vec![device(), device()]);
        assert_eq!(world.settle(), 0);
        // On the first device a room goes into the section and out again,
        // faster than its store is looked at: it sees the second write
        // only, which holds what was there before.
        world.devices[0]
            .sections
            .assign("!b", Some("Team"))
            .unwrap();
        assert!(world.send(0));
        world.devices[0].sections.assign("!b", None).unwrap();
        assert!(world.send(0));
        world.look(0);
        world.settle();
        assert_eq!(world.devices[1].sections[..], before[..]);
        // Later the other device puts the room there: the same content as
        // the first device's first write, and no echo of it.
        world.devices[1]
            .sections
            .assign("!b", Some("Team"))
            .unwrap();
        assert_eq!(world.settle(), 1);
        let after = [section("Team", &["!a", "!b"])];
        assert_eq!(world.devices[0].sections[..], after);
        assert_eq!(world.devices[1].sections[..], after);
    }

    #[test]
    fn a_device_that_writes_no_mark_is_heard_like_any_other() {
        // The other device runs mothrix 0.3.0: its writes say nothing of
        // who made them.
        let one = vec![String::new()];
        let before = vec![section("Team", &["!a"])];
        let mut world = World::new(
            before.clone(),
            vec![(OwnSections::new(before.clone()), one.clone())],
        );
        world.devices[0]
            .sections
            .assign("!b", Some("Team"))
            .unwrap();
        assert_eq!(world.settle(), 1);
        // It takes the room out again: what this device stored at first.
        world.stored.push(content(&before, None));
        assert_eq!(world.settle(), 0);
        assert_eq!(world.devices[0].sections[..], before[..]);
        // And what this version writes, 0.3.0 reads: the sections where
        // they were, the mark beside them.
        let written: serde_json::Value = serde_json::from_str(&world.stored[1]).unwrap();
        assert_eq!(written["sections"][0]["rooms"][1], "!b");
        assert_eq!(written["by"]["device"], "DEVICE0");
    }

    #[test]
    fn at_the_start_an_older_copy_in_the_store_is_no_news() {
        let old = content(&[section("Old", &["!a"])], None);
        let new = content(&[section("New", &["!a"])], Some(("PHONE", 7)));
        // The store still holds what the last run left; the server has
        // what the phone wrote since.
        let mut watch = Watch::new("HERE", Some(old.clone()));
        assert_eq!(
            watch.fetched(Some(new.clone())),
            Said::Sections(vec![section("New", &["!a"])])
        );
        // The first looks find the old copy, then the sync stores the
        // server's: neither is news.
        assert_eq!(watch.look(Some(old.clone()), 0), None);
        assert_eq!(watch.look(Some(new), 0), None);
        // The phone writes the old content again: that is.
        let again = content(&[section("Old", &["!a"])], Some(("PHONE", 8)));
        assert_eq!(
            watch.look(Some(again), 0),
            Some(Said::Sections(vec![section("Old", &["!a"])]))
        );
        // A reading of the store that failed says nothing.
        assert_eq!(watch.look(None, 0), None);
    }

    #[test]
    fn content_without_sections_is_unreadable_and_nothing_is_written_over_it() {
        for other in ["{}", r#"{"groups":[]}"#, "[]", r#"{"sections":7}"#] {
            assert_eq!(Said::of(Some(other)), Said::Unreadable, "{other}");
        }
        assert_eq!(Said::of(None), Said::Nothing);
        assert_eq!(
            Said::of(Some(r#"{"sections":[]}"#)),
            Said::Sections(Vec::new())
        );

        // It turns unreadable while running (a newer mothrix wrote): the
        // account is silent again, whatever changes here.
        let mut sync = Sync::default();
        let mut sections = OwnSections::new(vec![section("Team", &["!a"])]);
        sync.said(&mut sections, &prefixes(), 0, Said::Nothing);
        assert!(sync.to_send(&sections, &prefixes(), 0).is_some());
        let mut watch = Watch::new("HERE", None);
        watch.fetched(None);
        let said = watch.look(Some(r#"{"version":2}"#.into()), 0).unwrap();
        assert_eq!(said, Said::Unreadable);
        assert!(!sync.said(&mut sections, &prefixes(), 0, said));
        sections.assign("!b", Some("Team")).unwrap();
        assert!(sync.to_send(&sections, &prefixes(), 0).is_none());
        assert_eq!(sections[..], [section("Team", &["!a", "!b"])]);
    }

    #[test]
    fn a_server_that_could_not_be_asked_does_not_mean_the_account_stores_nothing() {
        // No network at the start, and nothing in the store (a new
        // sign-in): the account has not spoken, and nothing goes up.
        let mut watch = Watch::new("HERE", None);
        assert_eq!(watch.not_fetched(), None);
        assert!(!watch.has_told());
        // With something in the store, that is what is known.
        let stored = content(&[section("Team", &["!a"])], None);
        let mut offline = Watch::new("HERE", Some(stored.clone()));
        assert_eq!(
            offline.not_fetched(),
            Some(Said::Sections(vec![section("Team", &["!a"])]))
        );
        // The first sync stores the account's sections: now it has.
        assert!(matches!(
            watch.look(Some(stored), 0),
            Some(Said::Sections(_))
        ));
        assert!(watch.has_told());
    }

    #[test]
    fn an_older_write_of_this_device_coming_back_is_passed_over() {
        let first = content(&[section("Team", &["!a"])], Some(("HERE", 1)));
        let second = content(&[section("Team", &["!a", "!b"])], Some(("HERE", 2)));
        let mut watch = Watch::new("HERE", None);
        watch.fetched(None);
        // The sync brings the first write back after the second was sent.
        assert_eq!(watch.look(Some(first.clone()), 2), None);
        // The newest one is said: had another device written between the
        // two, this is what puts this device right again.
        assert!(watch.look(Some(second), 2).is_some());
        // The same content from another device, or from one that leaves
        // no mark, is that device's write.
        let phone = content(&[section("Team", &["!a"])], Some(("PHONE", 1)));
        assert!(watch.look(Some(phone), 2).is_some());
        let unmarked = content(&[section("Team", &["!a"])], None);
        assert!(watch.look(Some(unmarked), 2).is_some());
        // Numbers grow, also when the clock does not.
        assert_eq!(next_write(0, 1000), 1000);
        assert_eq!(next_write(1000, 1000), 1001);
        assert_eq!(next_write(1001, 900), 1002);
    }

    #[test]
    fn a_signed_out_account_s_rooms_leave_the_sections() {
        let mut own = OwnSections::new(vec![
            section("Team", &["!a", "work/!b"]),
            section("Clients", &["work/!d"]),
        ]);
        assert!(own.forget_account(&prefixes(), 1));
        assert_eq!(own[..], [section("Team", &["!a"])]);
        assert!(!own.forget_account(&prefixes(), 1));
        assert!(own.forget_account(&prefixes(), 0));
        assert!(own.is_empty());
        // And what was folded for it is not folded for the next account
        // of its name; other profiles keep theirs.
        let keys = ["work:Team", "Team", "@work", "@workshop", "workshop:Team"];
        let mut folds = Folds::new(keys.map(str::to_owned));
        assert!(folds.forget_profile("work"));
        assert_eq!(folds.keys(), ["@workshop", "Team", "workshop:Team"]);
        assert!(!folds.forget_profile("work"));
    }

    #[test]
    fn a_section_moved_on_one_device_moves_on_the_other_and_both_fall_silent() {
        let one = vec![String::new()];
        let before = vec![section("X", &["!x"]), section("Y", &["!y"])];
        let device = || (OwnSections::new(before.clone()), one.clone());
        let mut world = World::new(before.clone(), vec![device(), device()]);
        assert_eq!(world.settle(), 0);
        assert!(world.devices[0].sections.move_section("Y", false));
        assert_eq!(world.settle(), 1, "the move, once");
        let order = [section("Y", &["!y"]), section("X", &["!x"])];
        assert_eq!(world.devices[0].sections[..], order);
        assert_eq!(world.devices[1].sections[..], order);
        // And back, from the other device.
        assert!(world.devices[1].sections.move_section("Y", true));
        assert_eq!(world.settle(), 1);
        assert_eq!(world.devices[0].sections[..], before[..]);
    }

    #[test]
    fn a_parent_of_another_account_s_subsection_is_not_sent_back_and_forth() {
        // The terminal runs two accounts, the phone only the first.
        let both = OwnSections::new(vec![
            section("Team", &["!a"]),
            section("Work", &[]),
            section("Work/Clients", &["work/!d"]),
        ]);
        let stored = vec![section("Team", &["!a"]), section("Work", &[])];
        let mut world = World::new(
            stored,
            vec![
                (both.clone(), prefixes()),
                (OwnSections::default(), vec![String::new()]),
            ],
        );
        // What an older build stored (the parent, empty) is put right once.
        assert_eq!(world.settle(), 1);
        assert_eq!(world.devices[0].sections[..], both[..]);
        assert_eq!(world.devices[1].sections[..], [section("Team", &["!a"])]);
    }

    #[test]
    fn rooms_of_an_account_that_is_not_running_are_nobody_else_s() {
        // "work" signed out, or was left out at this start.
        let only_default = vec![String::new()];
        let sections = vec![
            section("Team", &["!a", "work/!b"]),
            section("Clients", &["work/!d"]),
        ];
        assert_eq!(
            share(&sections, &only_default, 0),
            [section("Team", &["!a"])]
        );
        // Nor are the default account's rooms the work account's, when
        // only that one runs.
        let only_work = vec!["work/".to_owned()];
        assert_eq!(
            share(&sections, &only_work, 0),
            [section("Team", &["!b"]), section("Clients", &["!d"])]
        );
        // What the running account stores leaves the other's rooms alone,
        // and names no room of another account.
        let mut sync = Sync::default();
        let mut own = OwnSections::new(sections);
        let theirs = vec![section("Team", &["!a", "work/!x", "!new"])];
        sync.heard(&mut own, &only_default, 0, Some(theirs));
        assert_eq!(
            own[..],
            [
                section("Team", &["work/!b", "!a", "!new"]),
                section("Clients", &["work/!d"])
            ]
        );
    }

    #[test]
    fn hearing_what_changes_nothing_here_is_no_change() {
        // Another account's section stands between: nothing to adopt.
        let mut sync = Sync::default();
        let mut sections = OwnSections::new(vec![
            section("Team", &["work/!b", "!a"]),
            section("Clients", &["work/!d"]),
        ]);
        let theirs = vec![section("Team", &["!a"]), section("Gone", &[])];
        assert!(!sync.heard(&mut sections, &prefixes(), 0, Some(theirs)));
    }

    #[test]
    fn a_share_from_elsewhere_moves_rooms_adds_and_drops_sections() {
        let mut sections = vec![
            section("Team", &["!a", "work/!b"]),
            section("Old", &["work/!x"]),
            section("Clients", &["work/!d"]),
        ];
        // The work account now has !b in Clients, a new section after it,
        // and no longer names Old.
        merge(
            &mut sections,
            &prefixes(),
            1,
            &[section("Clients", &["!d", "!b"]), section("Later", &["!e"])],
        );
        assert_eq!(
            sections,
            [
                section("Team", &["!a"]),
                section("Clients", &["work/!d", "work/!b"]),
                section("Later", &["work/!e"]),
            ]
        );
    }

    #[test]
    fn nothing_goes_out_before_the_account_spoke_and_local_sections_go_up_once() {
        let mut sync = Sync::default();
        let mut sections = OwnSections::new(vec![section("Team", &["!a"])]);
        assert!(sync.to_send(&sections, &prefixes(), 0).is_none());
        // The account stores nothing yet: what this device has goes up.
        assert!(!sync.heard(&mut sections, &prefixes(), 0, None));
        assert_eq!(
            sync.to_send(&sections, &prefixes(), 0),
            Some(vec![section("Team", &["!a"])])
        );
        assert!(sync.to_send(&sections, &prefixes(), 0).is_none());
        // A move here goes up; the same share coming back changes nothing.
        sections.assign("!b", Some("Team")).unwrap();
        let sent = sync.to_send(&sections, &prefixes(), 0);
        assert_eq!(sent, Some(vec![section("Team", &["!a", "!b"])]));
        assert!(!sync.heard(&mut sections, &prefixes(), 0, sent));
    }

    #[test]
    fn what_the_account_stores_wins_over_this_device_at_the_start() {
        let mut sync = Sync::default();
        let mut sections = OwnSections::new(vec![section("Mine", &["!a"])]);
        assert!(sync.heard(
            &mut sections,
            &prefixes(),
            0,
            Some(vec![section("Theirs", &["!a"])])
        ));
        assert_eq!(sections[..], [section("Theirs", &["!a"])]);
        assert!(sync.to_send(&sections, &prefixes(), 0).is_none());
    }

    #[test]
    fn what_an_account_stores_is_tidied_and_written_back() {
        // An older build stored a subsection without its parent, and an
        // emptied section.
        let mut sync = Sync::default();
        let mut sections = OwnSections::default();
        let stored = vec![section("Old", &[]), section("Work/Clients", &["!a"])];
        assert!(sync.heard(&mut sections, &prefixes(), 0, Some(stored)));
        let tidy = [section("Work", &[]), section("Work/Clients", &["!a"])];
        assert_eq!(sections[..], tidy);
        assert_eq!(sync.to_send(&sections, &prefixes(), 0), Some(tidy.to_vec()));
    }

    fn names(sections: &OwnSections) -> Vec<&str> {
        sections
            .iter()
            .map(|section| section.name.as_str())
            .collect()
    }

    fn accounts() -> Vec<Account> {
        ["default", "work"]
            .into_iter()
            .map(|profile| Account {
                profile: profile.into(),
                label: profile.into(),
                user_id: format!("@{profile}:x.org"),
                own_name: None,
                sync: crate::matrix::SyncStatus::Connecting,
                verified: None,
            })
            .collect()
    }

    #[test]
    fn rooms_go_into_sections_made_as_needed_and_emptied_ones_go() {
        let mut own = OwnSections::default();
        own.assign("!a", Some(" Work ")).unwrap();
        // The name matches regardless of case; a room is in one section.
        own.assign("!b", Some("work")).unwrap();
        own.assign("!a", Some("Family")).unwrap();
        assert_eq!(
            own[..],
            [section("Work", &["!b"]), section("Family", &["!a"])]
        );
        own.assign("!b", None).unwrap();
        own.assign("!a", Some("  ")).unwrap();
        assert!(own.is_empty());

        // Names that cannot be: nothing changes.
        own.assign("!a", Some("Family")).unwrap();
        for bad in ["Pinned", "low priority", "a/b/c", "Work/", "/"] {
            assert!(own.assign("!a", Some(bad)).is_err(), "{bad}");
        }
        assert_eq!(own[..], [section("Family", &["!a"])]);

        // A section that came this way from elsewhere can still be used.
        let mut own = OwnSections::new(vec![section("CHATS", &["!a"])]);
        own.assign("!b", Some("chats")).unwrap();
        assert_eq!(own[..], [section("CHATS", &["!a", "!b"])]);
    }

    #[test]
    fn subsections_get_their_parent_and_come_right_after_it() {
        let mut own = OwnSections::default();
        own.assign("!c", Some("Work / Clients")).unwrap();
        own.assign("!a", Some("Home")).unwrap();
        own.assign("!b", Some("work")).unwrap();
        assert_eq!(names(&own), ["Work", "Work/Clients", "Home"]);
        // The parent's spelling is taken over.
        own.assign("!d", Some("WORK/Suppliers")).unwrap();
        assert_eq!(
            names(&own),
            ["Work", "Work/Clients", "Work/Suppliers", "Home"]
        );
        // An emptied parent stays while a subsection holds a room.
        own.assign("!b", None).unwrap();
        assert_eq!(
            names(&own),
            ["Work", "Work/Clients", "Work/Suppliers", "Home"]
        );
        own.assign("!c", None).unwrap();
        own.assign("!d", None).unwrap();
        assert_eq!(names(&own), ["Home"]);
        assert_eq!(label("Work/Clients"), "Clients");
        assert_eq!(parent_name("Work/Clients"), Some("Work"));
    }

    #[test]
    fn renaming_takes_subsections_and_fold_state_along() {
        let accounts = accounts();
        let mut own = OwnSections::new(vec![
            section("Family", &["!f"]),
            section("Work", &["!w"]),
            section("Work/Clients", &["work/!c"]),
        ]);
        // Folded alone, and for the work account (several accounts).
        let mut folds = Folds::new(["Work".to_owned(), "work:Work/Clients".to_owned()]);
        assert_eq!(
            own.rename("Work", "Job", &mut folds, &accounts),
            Ok("Job".into())
        );
        assert_eq!(names(&own), ["Family", "Job", "Job/Clients"]);
        assert_eq!(folds.keys(), ["Job", "work:Job/Clients"]);

        // Taken names, regardless of case; empty ones.
        assert!(own.rename("Job", "family", &mut folds, &accounts).is_err());
        assert!(own.rename("Job", " ", &mut folds, &accounts).is_err());
        // A section with subsections cannot become one.
        assert!(
            own.rename("Job", "Family/Job", &mut folds, &accounts)
                .is_err()
        );
        assert!(
            own.rename("Nothing", "Else", &mut folds, &accounts)
                .is_err()
        );
        assert_eq!(names(&own), ["Family", "Job", "Job/Clients"]);

        // Another parent moves a subsection; a case change is a rename.
        own.rename("Job/Clients", "family/Clients", &mut folds, &accounts)
            .unwrap();
        assert_eq!(names(&own), ["Family", "Family/Clients", "Job"]);
        assert_eq!(folds.keys(), ["Job", "work:Family/Clients"]);
        own.rename("Job", "JOB", &mut folds, &accounts).unwrap();
        assert_eq!(names(&own), ["Family", "Family/Clients", "JOB"]);
    }

    #[test]
    fn removing_takes_subsections_and_their_fold_state() {
        let accounts = accounts();
        let mut own = OwnSections::new(vec![
            section("Work", &["!w"]),
            section("Work/Clients", &["!c"]),
            section("Family", &["!f"]),
        ]);
        let mut folds =
            Folds::new(["work:Work", "Work/Clients", "Family", "@work"].map(str::to_owned));
        own.remove("Work", &mut folds, &accounts);
        assert_eq!(names(&own), ["Family"]);
        assert_eq!(folds.keys(), ["@work", "Family"]);
    }

    #[test]
    fn sections_move_among_their_siblings() {
        let mut own = OwnSections::new(vec![
            section("School", &["!s"]),
            section("School/Maths", &["!m"]),
            section("School/Art", &["!a"]),
            section("Family", &["!f"]),
        ]);
        // Up past the subsections of the one before.
        assert!(own.move_section("Family", false));
        assert_eq!(
            names(&own),
            ["Family", "School", "School/Maths", "School/Art"]
        );
        assert!(!own.move_section("Family", false));
        assert!(own.move_section("School/Maths", true));
        assert_eq!(
            names(&own),
            ["Family", "School", "School/Art", "School/Maths"]
        );
        assert!(!own.move_section("School/Maths", true));
        assert!(!own.move_section("Nothing", true));
    }

    #[test]
    fn the_stored_form_is_plain_json() {
        let stored = Stored {
            sections: vec![section("Work/Clients", &["!a:example.org"])],
            by: None,
        };
        let json = serde_json::to_value(&stored).unwrap();
        assert_eq!(json["sections"][0]["name"], "Work/Clients");
        assert_eq!(json["sections"][0]["rooms"][0], "!a:example.org");
        assert_eq!(serde_json::from_value::<Stored>(json).unwrap(), stored);
    }
}
