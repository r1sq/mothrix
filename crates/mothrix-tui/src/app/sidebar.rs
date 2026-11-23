// SPDX-License-Identifier: MPL-2.0

//! The room list on the left: which rows it has, in which order, what
//! each shows and what a click on it does; folding, the account filter,
//! the "only unread" view, and the cursor.
//!
//! Drawing takes [`App::sidebar_lines`] as they are. How fold state is
//! keyed (per account, when there are several) stays in here.

use super::*;

pub use mothrix_core::sidebar::Row as SidebarRow;
use mothrix_core::sidebar::{Sidebar, Target};

/// Unread messages and mentions, shown at the end of a line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Badge {
    pub unread: usize,
    pub mentions: usize,
}

impl Badge {
    pub fn is_empty(self) -> bool {
        self.unread == 0 && self.mentions == 0
    }

    /// Mentions count before everything else.
    pub fn is_mention(self) -> bool {
        self.mentions > 0
    }

    /// " @2 " for mentions, else " 5 " for unread messages, else nothing.
    pub fn text(self) -> String {
        match (self.mentions, self.unread) {
            (0, 0) => String::new(),
            (0, unread) => format!(" {} ", unread.min(999)),
            (mentions, _) => format!(" @{} ", mentions.min(99)),
        }
    }
}

/// One line of the sidebar, ready to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarLine {
    pub row: SidebarRow,
    /// The section, account or room name (a subsection without its
    /// parent's name).
    pub label: String,
    /// A subsection, or a room in one.
    pub indent: bool,
    pub badge: Badge,
    /// A folded section or account.
    pub folded: bool,
    /// What a click on it does.
    pub action: HitAction,
}

impl App {
    /// The sidebar as it is drawn, line by line.
    pub fn sidebar_lines(&self) -> Vec<SidebarLine> {
        self.sidebar()
            .lines()
            .into_iter()
            .map(|line| SidebarLine {
                row: line.row,
                label: line.label,
                indent: line.indent,
                badge: Badge {
                    unread: line.unread,
                    mentions: line.mentions,
                },
                folded: line.folded,
                action: match line.target {
                    Target::Fold(key) => HitAction::ToggleSection(key),
                    Target::Room(id) => HitAction::OpenRoom(id),
                },
            })
            .collect()
    }

    /// Unread messages and mentions in all rooms of an account.
    pub fn account_unread(&self, account: usize) -> Badge {
        let (unread, mentions) = self.sidebar().account_unread(account);
        Badge { unread, mentions }
    }

    /// The user's own section a click target of the sidebar (see
    /// [`SidebarLine::action`]) folds, as an index into [`App::sections`].
    pub(super) fn own_section_of(&self, action: &HitAction) -> Option<usize> {
        let title = self
            .sidebar_lines()
            .into_iter()
            .find_map(|line| match line.row {
                SidebarRow::Header { title } | SidebarRow::Folded { title, .. }
                    if line.action == *action =>
                {
                    Some(title)
                }
                _ => None,
            })?;
        self.state
            .sections
            .iter()
            .position(|section| section.name == title)
    }

    /// Scroll the sidebar so that `height` lines of `lines` show the
    /// cursor, but only when the cursor moved, so that the mouse wheel can
    /// scroll the list freely.
    pub fn follow_sidebar_cursor(&mut self, lines: &[SidebarLine], height: usize) {
        if self.cursor_room != self.sidebar_cursor_seen {
            self.sidebar_cursor_seen = self.cursor_room.clone();
            let cursor_row = self.cursor_room.as_ref().and_then(|id| {
                lines.iter().position(
                    |line| matches!(&line.action, HitAction::OpenRoom(room) if room == id),
                )
            });
            if let Some(row) = cursor_row {
                if row < self.sidebar_scroll + 1 {
                    self.sidebar_scroll = row.saturating_sub(1);
                } else if row >= self.sidebar_scroll + height {
                    self.sidebar_scroll = row + 1 - height;
                }
            }
        }
        self.sidebar_scroll = self.sidebar_scroll.min(lines.len().saturating_sub(height));
    }

    /// Keep the cursor on a listed room: when its room was folded away,
    /// filtered out or is gone, it moves to the first one listed.
    pub(super) fn keep_cursor_listed(&mut self) {
        let ids = self.visible_room_ids();
        if !self.cursor_room.as_ref().is_some_and(|id| ids.contains(id)) {
            self.cursor_room = ids.first().cloned();
        }
    }

    /// What the room list is made from, as it is now.
    fn sidebar(&self) -> Sidebar<'_> {
        Sidebar {
            rooms: &self.state.rooms,
            accounts: &self.state.accounts,
            sections: &self.state.sections,
            prefs: &self.state.prefs,
            folded: &self.state.folded,
            filter: self.filter.text(),
            unread_only: self.state.unread_only,
            account_filter: self.state.account_filter,
            open_room: self.state.open_room.as_deref(),
        }
    }

    /// The title of the sidebar section a room is listed in.
    fn section_of(&self, room: &RoomEntry) -> String {
        self.sidebar().section_of(room)
    }

    /// Show all accounts, or only one.
    pub(crate) fn set_account_filter(&mut self, filter: Option<usize>) {
        self.state.account_filter = filter.filter(|account| *account < self.state.accounts.len());
        self.sidebar_scroll = 0;
        self.state.config_dirty = true;
        self.keep_cursor_listed();
    }

    /// List only chats with something unread, or everything again.
    pub(crate) fn toggle_unread_only(&mut self) {
        self.state.unread_only = !self.state.unread_only;
        self.sidebar_scroll = 0;
        self.keep_cursor_listed();
    }

    /// All accounts, then each one alone, then all again.
    pub(super) fn next_account_filter(&mut self) {
        if self.state.accounts.len() < 2 {
            self.notice =
                Some("Only one account; add one in the settings (F2, Accounts).".to_owned());
            return;
        }
        let next = match self.state.account_filter {
            None => Some(0),
            Some(account) if account + 1 < self.state.accounts.len() => Some(account + 1),
            Some(_) => None,
        };
        self.set_account_filter(next);
    }

    /// Sidebar contents in display order (see [`Sidebar::rows`]).
    pub(super) fn sidebar_rows(&self) -> Vec<SidebarRow> {
        self.sidebar().rows()
    }

    /// The name the fold state of a section is kept under.
    fn fold_key(&self, account: Option<usize>, title: &str) -> String {
        self.sidebar().fold_key(account, title)
    }

    /// Fold or unfold a sidebar section.
    pub(crate) fn toggle_section(&mut self, title: &str) {
        if self.state.unread_only {
            return;
        }
        self.state.folded.toggle(title);
        self.state.config_dirty = true;
        self.keep_cursor_listed();
    }

    /// Fold the section the sidebar cursor is in.
    pub(super) fn fold_cursor_section(&mut self) {
        let title = self
            .cursor_room
            .as_ref()
            .and_then(|id| self.state.rooms.iter().find(|room| room.id == *id))
            .map(|room| self.fold_key(Some(room.account), &self.section_of(room)));
        match title {
            Some(title) => self.toggle_section(&title),
            None => self.notice = Some("Move to a room first; Z unfolds everything.".to_owned()),
        }
    }

    /// Show rooms and people in one section, or in two.
    pub(crate) fn toggle_mixed_list(&mut self) {
        self.state.prefs.mixed_list = !self.state.prefs.mixed_list;
        self.sidebar_scroll = 0;
        self.state.config_dirty = true;
    }

    pub(super) fn visible_room_ids(&self) -> Vec<String> {
        self.sidebar_rows()
            .into_iter()
            .filter_map(|row| match row {
                SidebarRow::Room { index } => Some(self.state.rooms[index].id.clone()),
                SidebarRow::Header { title: _ }
                | SidebarRow::Folded { .. }
                | SidebarRow::Account { index: _ } => None,
            })
            .collect()
    }

    pub(super) fn move_cursor(&mut self, delta: isize) {
        let ids = self.visible_room_ids();
        if ids.is_empty() {
            self.cursor_room = None;
            return;
        }
        let current = self
            .cursor_room
            .as_ref()
            .and_then(|id| ids.iter().position(|candidate| candidate == id));
        let next = match current {
            Some(index) => (index as isize + delta).clamp(0, ids.len() as isize - 1) as usize,
            None if delta < 0 => ids.len() - 1,
            None => 0,
        };
        self.cursor_room = Some(ids[next].clone());
    }

    /// While filtering, keep the cursor on a room that is still listed,
    /// and the list at its top.
    pub(super) fn snap_cursor_to_filter(&mut self) {
        self.keep_cursor_listed();
        self.sidebar_scroll = 0;
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    use super::*;
    use crate::{model::Section, theme};

    fn room(id: &str, name: &str, account: usize) -> RoomEntry {
        RoomEntry {
            id: id.into(),
            name: name.into(),
            account,
            ..RoomEntry::default()
        }
    }

    /// Two accounts, "default" and "work", with an own section and a
    /// subsection in the mixed list.
    fn app() -> App {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        let mut work = app.state.accounts[0].clone();
        work.profile = "work".into();
        work.label = "Work".into();
        app.state.accounts.push(work);
        app.state.prefs.mixed_list = true;
        app.state.sections = crate::sections::OwnSections::new(vec![
            Section {
                name: "Team".into(),
                rooms: vec!["work/!a".into()],
            },
            Section {
                name: "Team/Ops".into(),
                rooms: vec!["work/!b".into()],
            },
        ]);
        let mut rooms = vec![
            room("!x", "Lunch Club", 0),
            room("work/!a", "Standup", 1),
            room("work/!b", "Pager", 1),
        ];
        rooms[1].unread = 2;
        rooms[2].mentions = 1;
        app.on_update(Update::Rooms(rooms));
        app
    }

    fn line<'a>(lines: &'a [SidebarLine], label: &str) -> &'a SidebarLine {
        lines
            .iter()
            .find(|line| line.label == label)
            .unwrap_or_else(|| panic!("no line {label:?} in {lines:#?}"))
    }

    #[test]
    fn lines_carry_what_is_drawn() {
        let app = app();
        let lines = app.sidebar_lines();
        let labels: Vec<_> = lines.iter().map(|line| line.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "default",
                "CHATS",
                "Lunch Club",
                "Work",
                "Team",
                "Standup",
                "Ops",
                "Pager",
            ]
        );
        // A subsection and its rooms are indented, without the parent's name.
        assert!(line(&lines, "Ops").indent && line(&lines, "Pager").indent);
        assert!(!line(&lines, "Team").indent && !line(&lines, "Standup").indent);
        // Rooms and accounts carry their unread counts.
        assert_eq!(line(&lines, "Pager").badge.text(), " @1 ");
        assert_eq!(
            line(&lines, "Work").badge,
            Badge {
                unread: 2,
                mentions: 1
            }
        );
        assert!(line(&lines, "default").badge.is_empty());
        assert_eq!(
            line(&lines, "Standup").action,
            HitAction::OpenRoom("work/!a".into())
        );
    }

    #[test]
    fn a_section_folds_for_its_own_account_and_shows_what_is_inside() {
        let mut app = app();
        // Both accounts have a "CHATS"; fold it only for "work".
        app.on_update(Update::Rooms(vec![
            room("!x", "Lunch Club", 0),
            room("work/!c", "Coffee", 1),
        ]));
        app.state.sections = Default::default();
        let lines = app.sidebar_lines();
        let chats: Vec<_> = lines.iter().filter(|line| line.label == "CHATS").collect();
        assert_eq!(chats.len(), 2);
        app.on_hit(chats[1].action.clone(), &mut Vec::new());

        let lines = app.sidebar_lines();
        let chats: Vec<_> = lines.iter().filter(|line| line.label == "CHATS").collect();
        assert!(!chats[0].folded && chats[1].folded);
        assert!(lines.iter().any(|line| line.label == "Lunch Club"));
        assert!(!lines.iter().any(|line| line.label == "Coffee"));
    }

    #[test]
    fn a_folded_parent_counts_its_subsections() {
        let mut app = app();
        let team = line(&app.sidebar_lines(), "Team").action.clone();
        app.on_hit(team, &mut Vec::new());
        let lines = app.sidebar_lines();
        let team = line(&lines, "Team");
        assert!(team.folded);
        assert_eq!(
            team.badge,
            Badge {
                unread: 2,
                mentions: 1
            }
        );
        assert!(!lines.iter().any(|line| line.label == "Ops"));
    }

    #[test]
    fn right_clicking_an_own_section_of_a_second_account_opens_its_manager() {
        let mut app = app();
        crate::ui::render_to_text(&mut app, 100, 30);
        let ops = app
            .hits
            .iter()
            .find(|hit| hit.action == line(&app.sidebar_lines(), "Ops").action)
            .expect("the Ops header on screen")
            .rect;
        app.on_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: ops.x + 2,
            row: ops.y,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(
            app.overlays.get::<SectionManager>().map(|m| m.cursor),
            Some(1)
        );
    }

    #[test]
    fn the_unread_view_names_the_account_of_each_room() {
        let mut app = app();
        app.toggle_unread_only();
        let labels: Vec<_> = app
            .sidebar_lines()
            .into_iter()
            .map(|line| (line.label, line.indent))
            .collect();
        assert_eq!(
            labels,
            [
                ("UNREAD".to_owned(), false),
                ("Pager · Work".to_owned(), false),
                ("Standup · Work".to_owned(), false),
            ]
        );
    }

    #[test]
    fn the_cursor_moves_on_when_its_room_goes_away() {
        let mut app = app();
        app.cursor_room = Some("work/!b".into());
        app.on_update(Update::Rooms(vec![
            room("!x", "Lunch Club", 0),
            room("work/!a", "Standup", 1),
        ]));
        assert_eq!(app.cursor_room.as_deref(), Some("!x"));
    }

    #[test]
    fn the_list_scrolls_to_the_cursor_only_when_it_moved() {
        let mut app = app();
        let lines = app.sidebar_lines();
        app.cursor_room = Some("work/!b".into());
        app.follow_sidebar_cursor(&lines, 3);
        assert_eq!(app.sidebar_scroll, 5);
        // The wheel scrolls freely while the cursor stays.
        app.sidebar_scroll = 0;
        app.follow_sidebar_cursor(&lines, 3);
        assert_eq!(app.sidebar_scroll, 0);
    }
}
