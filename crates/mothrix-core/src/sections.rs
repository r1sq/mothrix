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

/// The account data's content.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    #[serde(default)]
    pub sections: Vec<Section>,
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
        let mut own = Self(sections);
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
                    let (owner, id) = split(prefixes, key);
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
    let mine = |key: &String| split(prefixes, key).0 == account;
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
    /// What was sent lately, newest last. A sync can bring back an older
    /// one of these after a newer was sent; that echo is no news and must
    /// not undo the newer.
    sent: Vec<Vec<Section>>,
}

/// How many sent shares are remembered as possible echoes.
const ECHOES: usize = 8;

impl Sync {
    /// The account (at `account` in `prefixes`) says what it stores
    /// (`None`: nothing yet). Returns whether `sections` changed. What it
    /// stores is tidied on the way in; when that changed it,
    /// [`Sync::to_send`] writes the tidy form back.
    pub fn heard(
        &mut self,
        sections: &mut OwnSections,
        prefixes: &[String],
        account: usize,
        stored: Option<Vec<Section>>,
    ) -> bool {
        // An older write of ours coming back: the newer one stands.
        if let Some(theirs) = &stored
            && self.sent.last() != Some(theirs)
            && self.sent.contains(theirs)
        {
            return false;
        }
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
        self.sent.push(now.clone());
        if self.sent.len() > ECHOES {
            self.sent.remove(0);
        }
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

    /// Devices of one account and what it stores: each hears every write
    /// (its own too) in the order the server took them, and sends what it
    /// then has to say.
    struct World {
        devices: Vec<(OwnSections, Sync, Vec<String>)>,
        stored: Vec<Vec<Section>>,
        heard: Vec<usize>,
    }

    impl World {
        fn new(stored: Vec<Section>, devices: Vec<(OwnSections, Vec<String>)>) -> Self {
            World {
                heard: vec![0; devices.len()],
                devices: devices
                    .into_iter()
                    .map(|(sections, prefixes)| (sections, Sync::default(), prefixes))
                    .collect(),
                stored: vec![stored],
            }
        }

        /// Run until all are silent; the number of writes that took.
        fn settle(&mut self) -> usize {
            let before = self.stored.len();
            for _ in 0..20 {
                let mut quiet = true;
                for (index, (sections, sync, prefixes)) in self.devices.iter_mut().enumerate() {
                    while self.heard[index] < self.stored.len() {
                        let stored = self.stored[self.heard[index]].clone();
                        sync.heard(sections, prefixes, 0, Some(stored));
                        self.heard[index] += 1;
                    }
                    if let Some(share) = sync.to_send(sections, prefixes, 0) {
                        self.stored.push(share);
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
    fn a_section_moved_on_one_device_moves_on_the_other_and_both_fall_silent() {
        let one = vec![String::new()];
        let before = vec![section("X", &["!x"]), section("Y", &["!y"])];
        let device = || (OwnSections::new(before.clone()), one.clone());
        let mut world = World::new(before.clone(), vec![device(), device()]);
        assert_eq!(world.settle(), 0);
        assert!(world.devices[0].0.move_section("Y", false));
        assert_eq!(world.settle(), 1, "the move, once");
        let order = [section("Y", &["!y"]), section("X", &["!x"])];
        assert_eq!(world.devices[0].0[..], order);
        assert_eq!(world.devices[1].0[..], order);
        // And back, from the other device.
        assert!(world.devices[1].0.move_section("Y", true));
        assert_eq!(world.settle(), 1);
        assert_eq!(world.devices[0].0[..], before[..]);
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
        assert_eq!(world.devices[0].0[..], both[..]);
        assert_eq!(world.devices[1].0[..], [section("Team", &["!a"])]);
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
    fn an_older_write_coming_back_does_not_undo_a_newer() {
        let mut sync = Sync::default();
        let mut sections = OwnSections::new(vec![section("Team", &["!a"])]);
        sync.heard(&mut sections, &prefixes(), 0, None);
        let first = sync.to_send(&sections, &prefixes(), 0).unwrap();
        sections.assign("!b", Some("Team")).unwrap();
        sync.to_send(&sections, &prefixes(), 0);
        // The sync brings the first write back after the second was sent.
        assert!(!sync.heard(&mut sections, &prefixes(), 0, Some(first)));
        assert_eq!(sections[..], [section("Team", &["!a", "!b"])]);
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
        };
        let json = serde_json::to_value(&stored).unwrap();
        assert_eq!(json["sections"][0]["name"], "Work/Clients");
        assert_eq!(json["sections"][0]["rooms"][0], "!a:example.org");
        assert_eq!(serde_json::from_value::<Stored>(json).unwrap(), stored);
    }
}
