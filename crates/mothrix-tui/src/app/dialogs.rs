// SPDX-License-Identifier: MPL-2.0

//! The search dialog, own sections, the room menu, and spell checking of
//! the message box.

use std::sync::Arc;

use super::*;
use crate::{
    model::{GrammarMatch, SearchHit},
    spell::{self, Speller},
};

/// The menu a right-click on a room opens.
#[derive(Clone, Debug, Default)]
pub struct RoomMenu {
    pub room_id: String,
    /// Where the click was.
    pub x: u16,
    pub y: u16,
    pub cursor: usize,
}

/// An entry of a room's right-click menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomMenuItem {
    Open,
    Pin,
    Section,
    Priority,
    Notify,
    Info,
    Leave,
}

/// A row of the section picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectionRow {
    /// Into this section.
    Section(String),
    /// Into a new one, named next.
    New,
    /// Out of the user's sections.
    Out,
    /// To the dialog for renaming and reordering them.
    Manage,
}

/// The dialog for renaming, reordering and removing the user's sections.
#[derive(Clone, Debug, Default)]
pub struct SectionManager {
    pub cursor: usize,
    /// The new name being typed for the section under the cursor.
    pub renaming: Option<Input>,
}

/// A change made in the section manager, to the section at this position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionEdit {
    Select(usize),
    Up(usize),
    Down(usize),
    Rename(usize),
    Delete(usize),
}

/// The dialog for putting a room into one of the user's own sections.
#[derive(Clone, Debug, Default)]
pub struct SectionPicker {
    pub room_id: String,
    pub cursor: usize,
    /// The name of a new section being typed.
    pub naming: Option<Input>,
}

/// The corrections offered for the marked text under the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpellPopup {
    start: usize,
    end: usize,
    suggestions: Vec<String>,
    /// What LanguageTool says is wrong (none for the built-in check).
    message: Option<String>,
    /// A word that can be added to the ignore list.
    ignorable: bool,
    /// The arrow keys chose a correction, so Enter applies it.
    engaged: bool,
}

/// The search dialog.
#[derive(Clone, Debug, Default)]
pub struct Search {
    pub input: Input,
    /// Search every chat instead of only the open one.
    pub all_chats: bool,
    pub hits: Vec<SearchHit>,
    pub selected: usize,
    /// The (query, scope) the current `hits` belong to.
    pub searched: Option<(String, bool)>,
    pub waiting: bool,
}

impl App {
    // -- settings -------------------------------------------------------------

    /// Use the LanguageTool server at `url`; empty switches it off.
    pub fn set_languagetool_url(&mut self, url: String) {
        self.state.prefs.languagetool_url = url;
        self.grammar.clear();
        self.grammar_checked = None;
        self.grammar_requested = None;
        self.grammar_failed = false;
        self.spell_key = None;
        self.refresh_spelling();
    }

    // -- own sections ---------------------------------------------------------

    /// Rows of the section picker: (label, the room is in it).
    pub fn section_picker_rows(&self) -> Vec<(String, bool)> {
        let Some(picker) = self.overlays.get::<SectionPicker>() else {
            return Vec::new();
        };
        let mut rows: Vec<(String, bool)> = self
            .state
            .sections
            .iter()
            .map(|section| {
                (
                    section.name.clone(),
                    section.rooms.contains(&picker.room_id),
                )
            })
            .collect();
        let in_one = rows.iter().any(|(_, current)| *current);
        rows.push(("New section…".to_owned(), false));
        if in_one {
            rows.push(("No section".to_owned(), false));
        }
        if !self.state.sections.is_empty() {
            rows.push(("Rename or reorder sections…".to_owned(), false));
        }
        rows
    }

    /// Ask which section the room under the sidebar cursor should go in.
    pub(super) fn open_section_picker(&mut self) {
        let Some(room_id) = self.cursor_room.clone() else {
            self.notice = Some("Move to a room first.".to_owned());
            return;
        };
        if !self.state.prefs.mixed_list {
            // Own sections live in the mixed list.
            self.toggle_mixed_list();
            self.notice =
                Some("Your own sections are part of the mixed list; switched to it.".to_owned());
        }
        self.overlays.open(SectionPicker {
            room_id,
            ..SectionPicker::default()
        });
    }

    /// Put a room into the section `name` (made if new), or into none.
    /// Returns whether it went; if not, the notice says why.
    pub fn assign_section(&mut self, room_id: &str, name: Option<&str>) -> bool {
        match self.state.sections.assign(room_id, name) {
            Ok(()) => {
                self.state.config_dirty = true;
                true
            }
            Err(reason) => {
                self.notice = Some(reason);
                false
            }
        }
    }

    // -- the right-click menu of a room ---------------------------------------

    pub fn room_menu_rows(&self) -> Vec<(RoomMenuItem, String)> {
        let room = self
            .overlays
            .get::<RoomMenu>()
            .and_then(|menu| self.state.rooms.iter().find(|room| room.id == menu.room_id));
        let Some(room) = room else {
            return Vec::new();
        };
        let pin = if room.favourite {
            "Unpin"
        } else {
            "Pin to top"
        };
        let priority = if room.low_priority {
            "Normal priority"
        } else {
            "Low priority"
        };
        vec![
            (RoomMenuItem::Open, "Open".to_owned()),
            (RoomMenuItem::Pin, pin.to_owned()),
            (RoomMenuItem::Section, "Move to section…".to_owned()),
            (RoomMenuItem::Priority, priority.to_owned()),
            // Choosing it goes on to the next setting.
            (
                RoomMenuItem::Notify,
                format!("Notify: {}", room.notify.label()),
            ),
            (RoomMenuItem::Info, "Room info…".to_owned()),
            (RoomMenuItem::Leave, "Leave…".to_owned()),
        ]
    }

    pub(super) fn pick_room_menu(&mut self, item: RoomMenuItem, commands: &mut Vec<Command>) {
        let Some(menu) = self.overlays.take::<RoomMenu>() else {
            return;
        };
        match item {
            RoomMenuItem::Open => self.open(menu.room_id, commands),
            RoomMenuItem::Pin => self.toggle_pin(&menu.room_id, commands),
            RoomMenuItem::Section => {
                self.cursor_room = Some(menu.room_id);
                self.open_section_picker();
            }
            RoomMenuItem::Priority => self.toggle_low_priority(&menu.room_id, commands),
            RoomMenuItem::Notify => {
                if let Some(room) = self
                    .state
                    .rooms
                    .iter_mut()
                    .find(|room| room.id == menu.room_id)
                {
                    // Shown at once; the server's word follows.
                    room.notify = room.notify.next();
                    commands.push(Command::SetNotify {
                        room_id: menu.room_id.clone(),
                        mode: room.notify,
                    });
                    self.notice = Some(format!("Notifications: {}.", room.notify.label()));
                }
                // Stays open, to go on to the next choice.
                self.overlays.open(menu);
            }
            RoomMenuItem::Info => self.open_room_info(menu.room_id, commands),
            RoomMenuItem::Leave => {
                self.open_room_info(menu.room_id, commands);
                if let Some(info) = self.overlays.get_mut::<RoomInfo>() {
                    info.mode =
                        super::roominfo::InfoMode::Confirm(super::roominfo::InfoConfirm::Leave);
                }
            }
        }
    }

    pub(super) fn on_room_menu_key(&mut self, key: KeyEvent, commands: &mut Vec<Command>) {
        let items: Vec<RoomMenuItem> = self
            .room_menu_rows()
            .into_iter()
            .map(|(item, _)| item)
            .collect();
        let rows = items.len().max(1);
        let Some(menu) = self.overlays.get_mut::<RoomMenu>() else {
            return;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                menu.cursor = menu.cursor.checked_sub(1).unwrap_or(rows - 1);
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                menu.cursor = (menu.cursor + 1) % rows;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(item) = items.get(menu.cursor).copied() {
                    self.pick_room_menu(item, commands);
                }
            }
            _ => self.overlays.close::<RoomMenu>(),
        }
    }

    /// What row `index` of the section picker does.
    pub fn section_picker_row(&self, index: usize) -> Option<SectionRow> {
        let rows = self.section_picker_rows().len();
        let count = self.state.sections.len();
        Some(if index < count {
            SectionRow::Section(self.state.sections[index].name.clone())
        } else if index == count {
            SectionRow::New
        } else if count > 0 && index + 1 == rows {
            SectionRow::Manage
        } else if index < rows {
            SectionRow::Out
        } else {
            return None;
        })
    }

    pub(super) fn pick_section(&mut self, row: SectionRow) {
        let new_row = self.state.sections.len();
        let Some(picker) = self.overlays.get_mut::<SectionPicker>() else {
            return;
        };
        let room_id = picker.room_id.clone();
        match row {
            SectionRow::Section(name) => {
                self.assign_section(&room_id, Some(&name));
                self.overlays.close::<SectionPicker>();
            }
            SectionRow::New => {
                picker.cursor = new_row;
                picker.naming = Some(Input::default());
            }
            SectionRow::Manage => self.open_section_manager(0),
            SectionRow::Out => {
                self.assign_section(&room_id, None);
                self.overlays.close::<SectionPicker>();
            }
        }
    }

    // -- renaming and reordering sections -------------------------------------

    pub(crate) fn open_section_manager(&mut self, at: usize) {
        if self.state.sections.is_empty() {
            self.notice = Some("No sections yet: press g on a room to make one.".to_owned());
            return;
        }
        self.overlays.close::<SectionPicker>();
        self.overlays.open(SectionManager {
            cursor: at.min(self.state.sections.len() - 1),
            renaming: None,
        });
    }

    pub(crate) fn edit_section(&mut self, edit: SectionEdit) {
        if !self.overlays.is_open::<SectionManager>() {
            return;
        }
        let (SectionEdit::Select(index)
        | SectionEdit::Up(index)
        | SectionEdit::Down(index)
        | SectionEdit::Rename(index)
        | SectionEdit::Delete(index)) = edit;
        let Some(name) = self
            .state
            .sections
            .get(index)
            .map(|section| section.name.clone())
        else {
            return;
        };
        let mut renaming = None;
        match edit {
            SectionEdit::Select(_) => {}
            SectionEdit::Up(_) | SectionEdit::Down(_) => {
                let down = matches!(edit, SectionEdit::Down(_));
                if self.state.sections.move_section(&name, down) {
                    self.state.config_dirty = true;
                }
            }
            SectionEdit::Rename(_) => {
                let mut input = Input::default();
                input.set(&name);
                renaming = Some(input);
            }
            // Its chats, and those of its subsections, go back to the
            // general list.
            SectionEdit::Delete(_) => {
                self.state
                    .sections
                    .remove(&name, &mut self.state.folded, &self.state.accounts);
                self.state.config_dirty = true;
            }
        }
        if self.state.sections.is_empty() {
            self.overlays.close::<SectionManager>();
            return;
        }
        // The cursor stays on the section that was acted on.
        let cursor = self
            .state
            .sections
            .iter()
            .position(|section| section.name == name)
            .unwrap_or(index.min(self.state.sections.len() - 1));
        self.overlays.open(SectionManager { cursor, renaming });
    }

    /// Give the section at `index` a new name; "Parent/Name" makes it a
    /// subsection (or moves it to another parent).
    fn rename_section(&mut self, index: usize, typed: &str) -> Result<String, String> {
        let old = self.state.sections[index].name.clone();
        let name = self.state.sections.rename(
            &old,
            typed,
            &mut self.state.folded,
            &self.state.accounts,
        )?;
        self.state.config_dirty = true;
        Ok(name)
    }

    pub(super) fn on_manager_key(&mut self, key: KeyEvent) {
        let count = self.state.sections.len();
        // Another device can take sections away while this is open.
        if count == 0 {
            self.overlays.close::<SectionManager>();
            return;
        }
        let Some(manager) = self.overlays.get_mut::<SectionManager>() else {
            return;
        };
        manager.cursor = manager.cursor.min(count - 1);
        let cursor = manager.cursor;
        if let Some(input) = manager.renaming.as_mut() {
            match key.code {
                KeyCode::Esc => manager.renaming = None,
                KeyCode::Enter => {
                    let typed = input.text().to_owned();
                    match self.rename_section(cursor, &typed) {
                        Ok(name) => {
                            let cursor = self
                                .state
                                .sections
                                .iter()
                                .position(|section| section.name == name)
                                .unwrap_or(0);
                            self.overlays.open(SectionManager {
                                cursor,
                                renaming: None,
                            });
                        }
                        Err(reason) => self.notice = Some(reason),
                    }
                }
                _ => {
                    input.edit(key);
                }
            }
            return;
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'G') => self.overlays.close::<SectionManager>(),
            KeyCode::Up if shift => self.edit_section(SectionEdit::Up(cursor)),
            KeyCode::Down if shift => self.edit_section(SectionEdit::Down(cursor)),
            KeyCode::Char('K') => self.edit_section(SectionEdit::Up(cursor)),
            KeyCode::Char('J') => self.edit_section(SectionEdit::Down(cursor)),
            KeyCode::Up | KeyCode::Char('k') => {
                manager.cursor = cursor.checked_sub(1).unwrap_or(count - 1);
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                manager.cursor = (cursor + 1) % count;
            }
            KeyCode::Enter | KeyCode::Char('r') => self.edit_section(SectionEdit::Rename(cursor)),
            KeyCode::Delete | KeyCode::Char('d') => self.edit_section(SectionEdit::Delete(cursor)),
            _ => {}
        }
    }

    pub(super) fn on_section_key(&mut self, key: KeyEvent) {
        let rows = self.section_picker_rows().len();
        let Some(picker) = self.overlays.get_mut::<SectionPicker>() else {
            return;
        };
        if let Some(input) = picker.naming.as_mut() {
            match key.code {
                KeyCode::Esc => picker.naming = None,
                KeyCode::Enter => {
                    let typed = input.text().trim().to_owned();
                    let room_id = picker.room_id.clone();
                    if typed.is_empty() {
                        picker.naming = None;
                    } else {
                        // Naming a section that exists puts the room there.
                        if self.assign_section(&room_id, Some(&typed)) {
                            self.overlays.close::<SectionPicker>();
                        }
                    }
                }
                _ => {
                    input.edit(key);
                }
            }
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'g') => self.overlays.close::<SectionPicker>(),
            KeyCode::Up | KeyCode::Char('k') => {
                picker.cursor = picker.cursor.checked_sub(1).unwrap_or(rows - 1);
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                picker.cursor = (picker.cursor + 1) % rows;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let index = picker.cursor;
                if let Some(row) = self.section_picker_row(index) {
                    self.pick_section(row);
                }
            }
            _ => {}
        }
    }

    // -- search ---------------------------------------------------------------

    /// Open the search dialog, for the open chat if there is one.
    pub(super) fn open_search(&mut self, query: &str) {
        let mut search = Search {
            all_chats: self.state.open_room.is_none(),
            ..Search::default()
        };
        search.input.set(query);
        self.overlays.close::<SettingsDialog>();
        self.overlays.open(search);
    }

    fn run_search(&mut self, commands: &mut Vec<Command>) {
        let open_room = self.state.open_room.clone();
        let Some(search) = self.overlays.get_mut::<Search>() else {
            return;
        };
        let query = search.input.text().trim().to_owned();
        if query.is_empty() {
            return;
        }
        // Without an open chat there is only "everywhere".
        search.all_chats |= open_room.is_none();
        search.searched = Some((query.clone(), search.all_chats));
        search.waiting = true;
        search.hits.clear();
        search.selected = 0;
        let room_id = if search.all_chats { None } else { open_room };
        commands.push(Command::Search { query, room_id });
    }

    pub(super) fn open_hit(
        &mut self,
        room_id: String,
        event_id: String,
        commands: &mut Vec<Command>,
    ) {
        if self.overlays.take::<Search>().is_none() {
            return;
        }
        // The open room again: out of its thread, where the message is not.
        self.open(room_id, commands);
        self.pending_reveal = Some(event_id.clone());
        self.notice = Some("Loading that message…".to_owned());
        commands.push(Command::Reveal { event_id });
        self.try_reveal();
    }

    pub(super) fn set_search_scope(&mut self, all_chats: bool) {
        if let Some(search) = self.overlays.get_mut::<Search>() {
            // "This chat" needs a chat.
            search.all_chats = all_chats || self.state.open_room.is_none();
        }
    }

    pub(super) fn on_search_key(&mut self, key: KeyEvent, commands: &mut Vec<Command>) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(search) = self.overlays.get_mut::<Search>() else {
            return;
        };
        let count = search.hits.len();
        match key.code {
            KeyCode::Esc => self.overlays.close::<Search>(),
            KeyCode::Char('f') if ctrl => self.overlays.close::<Search>(),
            KeyCode::Tab => {
                let all = !search.all_chats;
                self.set_search_scope(all);
            }
            KeyCode::Up if count > 0 => search.selected = search.selected.saturating_sub(1),
            KeyCode::Down if count > 0 => search.selected = (search.selected + 1).min(count - 1),
            KeyCode::PageUp if count > 0 => search.selected = search.selected.saturating_sub(10),
            KeyCode::PageDown if count > 0 => {
                search.selected = (search.selected + 10).min(count - 1)
            }
            KeyCode::Enter => {
                let current = (search.input.text().trim().to_owned(), search.all_chats);
                // Enter searches; on unchanged results it opens the chosen one.
                if search.searched.as_ref() == Some(&current) && !search.waiting && count > 0 {
                    if let Some(hit) = search.hits.get(search.selected) {
                        let (room_id, event_id) = (hit.room_id.clone(), hit.event_id.clone());
                        self.open_hit(room_id, event_id, commands);
                    }
                } else {
                    self.run_search(commands);
                }
            }
            _ => {
                search.input.edit(key);
            }
        }
    }

    pub(super) fn on_search_results(&mut self, query: String, hits: Vec<SearchHit>) {
        if let Some(search) = self.overlays.get_mut::<Search>()
            && search
                .searched
                .as_ref()
                .is_some_and(|(searched, _)| *searched == query)
        {
            search.hits = hits;
            search.selected = 0;
            search.waiting = false;
        }
    }

    /// Select the message a search result pointed at, once it is loaded.
    pub(super) fn try_reveal(&mut self) {
        let Some(event_id) = self.pending_reveal.clone() else {
            return;
        };
        if self
            .state
            .items
            .iter()
            .any(|item| item.event_id == event_id)
        {
            self.pending_reveal = None;
            self.notice = None;
            self.select(event_id);
            self.viewport.reveal_selection();
        }
    }

    // -- spell checking -------------------------------------------------------

    /// Hand over the loaded dictionaries (they load in the background).
    pub fn set_speller(&mut self, speller: Arc<dyn Speller>, languages: Vec<String>) {
        self.speller = Some(speller);
        self.spell_languages = languages;
        self.spell_key = None;
        self.refresh_spelling();
    }

    fn is_ignored_word(&self, word: &str) -> bool {
        let lower = word.to_lowercase();
        self.ignored_words.contains(&lower)
            // Names of the people in the room are not typos.
            || self.state.members.iter().any(|member| {
                member.name.split_whitespace().any(|part| part.to_lowercase() == lower)
            })
    }

    // -- LanguageTool ---------------------------------------------------------

    fn languagetool_active(&self) -> bool {
        self.state.prefs.spellcheck && !self.state.prefs.languagetool_url.trim().is_empty()
    }

    /// The message text to send to the LanguageTool server, if a server is
    /// set and the text changed since the last call.
    pub(super) fn grammar_request(&mut self) -> Option<String> {
        if !self.languagetool_active() {
            return None;
        }
        let text = self.composer.text();
        // Nothing to check in an empty box or a command.
        if text.trim().is_empty() || text.starts_with('/') {
            self.grammar_requested = None;
            return None;
        }
        if self.grammar_requested.as_deref() == Some(text) {
            return None;
        }
        self.grammar_requested = Some(text.to_owned());
        self.grammar_requested.clone()
    }

    /// The server's answer for `text`. Answers for anything but the latest
    /// request are dropped.
    pub fn set_grammar(&mut self, text: String, result: Result<Vec<GrammarMatch>, String>) {
        if self.grammar_requested.as_deref() != Some(text.as_str()) {
            return;
        }
        match result {
            Ok(matches) => {
                self.grammar = matches;
                self.grammar_checked = Some(text);
                self.grammar_failed = false;
            }
            Err(error) => {
                // Say so once, then quietly keep using the built-in check.
                if !self.grammar_failed {
                    self.notice = Some(format!(
                        "LanguageTool: {error}. Using the built-in spell check."
                    ));
                }
                self.grammar_failed = true;
                self.grammar.clear();
                self.grammar_checked = None;
            }
        }
        self.spell_key = None;
        self.refresh_spelling();
    }

    /// How many leading characters of `text` the last LanguageTool answer
    /// still describes: the part unchanged since, minus a word that was
    /// changed at its end.
    fn grammar_coverage(&self, text: &str) -> usize {
        let Some(checked) = self
            .grammar_checked
            .as_deref()
            .filter(|_| self.languagetool_active())
        else {
            return 0;
        };
        let now: Vec<char> = text.chars().collect();
        let then: Vec<char> = checked.chars().collect();
        let mut prefix = now.iter().zip(&then).take_while(|(a, b)| a == b).count();
        let word_goes_on = |chars: &[char]| chars.get(prefix).is_some_and(|c| !c.is_whitespace());
        if word_goes_on(&now) || word_goes_on(&then) {
            while prefix > 0 && !now[prefix - 1].is_whitespace() {
                prefix -= 1;
            }
        }
        prefix
    }

    // -- marking mistakes -----------------------------------------------------

    /// Re-check the message box if its text or cursor changed.
    pub(super) fn refresh_spelling(&mut self) {
        let key = (
            self.composer.text().to_owned(),
            self.composer.cursor(),
            self.state.prefs.spellcheck,
        );
        if self.spell_key.as_ref() == Some(&key) {
            return;
        }
        let (text, cursor) = (key.0.clone(), key.1);
        let same_text = self.spell_key.as_ref().is_some_and(|old| old.0 == text);
        self.spell_key = Some(key);
        self.misspelled.clear();
        self.grammar_marks.clear();
        if !self.state.prefs.spellcheck {
            self.spell_popup = None;
            return;
        }

        // LanguageTool's findings for the part of the text it has seen; the
        // built-in check covers what was typed since.
        let covered = self.grammar_coverage(&text);
        self.grammar.retain(|found| found.end <= covered);
        let grammar: Vec<GrammarMatch> = self
            .grammar
            .iter()
            .filter(|found| !(found.spelling && self.is_ignored_word(&found.text)))
            .cloned()
            .collect();
        for found in &grammar {
            let marks = if found.spelling {
                &mut self.misspelled
            } else {
                &mut self.grammar_marks
            };
            marks.push((found.start, found.end));
        }
        let speller = self.speller.clone();
        if let Some(speller) = &speller {
            let local = spell::misspelled(&text, cursor, speller.as_ref(), &|word| {
                self.is_ignored_word(word)
            });
            self.misspelled
                .extend(local.into_iter().filter(|(start, _)| *start >= covered));
        }
        self.misspelled.sort_unstable();

        // Offer corrections for the marked text the cursor is on.
        let on_cursor = |start: usize, end: usize| (start..=end).contains(&cursor);
        let from_server = grammar
            .iter()
            .find(|found| on_cursor(found.start, found.end));
        let range = from_server
            .map(|found| (found.start, found.end))
            .or_else(|| {
                self.misspelled
                    .iter()
                    .copied()
                    .find(|(start, end)| on_cursor(*start, *end))
            });
        self.spell_popup = match range {
            None => None,
            Some((start, end))
                if same_text
                    && self
                        .spell_popup
                        .as_ref()
                        .is_some_and(|popup| (popup.start, popup.end) == (start, end)) =>
            {
                self.spell_popup.take()
            }
            Some((start, end)) => {
                // New marked text: show the popup again even if the last one was closed.
                self.mention_hidden = false;
                Some(match from_server {
                    Some(found) => SpellPopup {
                        start,
                        end,
                        suggestions: found.replacements.iter().take(5).cloned().collect(),
                        message: Some(found.message.clone()),
                        ignorable: found.spelling,
                        engaged: false,
                    },
                    None => {
                        let word: String = text.chars().skip(start).take(end - start).collect();
                        SpellPopup {
                            start,
                            end,
                            suggestions: speller
                                .as_ref()
                                .map(|speller| speller.suggest(&word))
                                .unwrap_or_default(),
                            message: None,
                            ignorable: true,
                            engaged: false,
                        }
                    }
                })
            }
        };
    }

    /// Whether a correction was chosen with the arrow keys (until then
    /// Enter sends the message).
    pub(super) fn spelling_engaged(&self) -> bool {
        self.spell_popup.as_ref().is_some_and(|popup| popup.engaged)
    }

    pub(super) fn engage_spelling(&mut self) {
        if let Some(popup) = self.spell_popup.as_mut() {
            popup.engaged = true;
        }
    }

    /// Title of the corrections popup: what is wrong, if the server said.
    pub(super) fn spelling_title(&self) -> String {
        match self
            .spell_popup
            .as_ref()
            .and_then(|popup| popup.message.as_deref())
        {
            Some(message) => {
                let short: String = message.chars().take(70).collect();
                let cut = if short.len() < message.len() {
                    "…"
                } else {
                    ""
                };
                format!(" {short}{cut} ")
            }
            None => " spelling · ↓ Enter, or Tab ".to_owned(),
        }
    }

    /// Rows of the corrections popup: the corrections, then "ignore".
    pub(super) fn spelling_rows(&self) -> Vec<(String, String)> {
        let Some(popup) = &self.spell_popup else {
            return Vec::new();
        };
        if self.mention_hidden || self.focus != Focus::Composer {
            return Vec::new();
        }
        let mut rows: Vec<(String, String)> = popup
            .suggestions
            .iter()
            .map(|suggestion| (suggestion.clone(), String::new()))
            .collect();
        if popup.ignorable {
            let word: String = self
                .composer
                .text()
                .chars()
                .skip(popup.start)
                .take(popup.end - popup.start)
                .collect();
            rows.push((format!("Ignore “{word}”"), "always".to_owned()));
        } else if rows.is_empty() {
            rows.push(("No suggestion".to_owned(), "Esc closes".to_owned()));
        }
        rows
    }

    pub(super) fn accept_spelling(&mut self, index: usize) {
        let Some(popup) = self.spell_popup.clone() else {
            return;
        };
        match popup.suggestions.get(index) {
            Some(suggestion) => self
                .composer
                .replace_range(popup.start, popup.end, suggestion),
            None if popup.ignorable => {
                let word: String = self
                    .composer
                    .text()
                    .chars()
                    .skip(popup.start)
                    .take(popup.end - popup.start)
                    .collect();
                let word = word.to_lowercase();
                if self.ignored_words.insert(word.clone()) {
                    self.effects.push(Effect::IgnoreWord(word));
                }
            }
            None => self.mention_hidden = true,
        }
        self.spell_key = None;
        self.mention_selected = 0;
        self.refresh_spelling();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::Member, spell::Words};

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

    fn room(id: &str, name: &str) -> RoomEntry {
        RoomEntry {
            id: id.into(),
            name: name.into(),
            ..RoomEntry::default()
        }
    }

    /// Room `!a` open with two messages.
    fn chat() -> App {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.on_update(Update::Rooms(vec![
            room("!a", "Lunch Club"),
            room("!b", "Dev Chat"),
        ]));
        app.on_event(key(KeyCode::Enter));
        let message = |id: &str, body: &str| Item {
            key: id.into(),
            event_id: id.into(),
            body: body.into(),
            ..Item::default()
        };
        app.on_update(Update::Timeline {
            room_id: "!a".into(),
            items: vec![message("$1", "first"), message("$2", "second")],
        });
        app
    }

    fn hit(room_id: &str, event_id: &str, text: &str) -> SearchHit {
        SearchHit {
            room_id: room_id.into(),
            room_name: "Room".into(),
            event_id: event_id.into(),
            sender: "Amy".into(),
            ts_ms: 1,
            text: text.into(),
        }
    }

    #[test]
    fn settings_dialog_changes_and_marks_the_config_for_saving() {
        let mut app = chat();
        app.on_event(key(KeyCode::F(2)));
        assert!(app.overlays.is_open::<SettingsDialog>());
        // Typing does not reach the message box while the dialog is open.
        type_text(&mut app, "x");
        assert!(app.composer.is_empty());

        assert_eq!(app.setting_value(Setting::Theme), "mothrix");
        app.on_event(key(KeyCode::Enter));
        assert_eq!(app.theme.name, "tokyo-night");
        app.on_event(key(KeyCode::Left));
        assert_eq!(app.theme.name, "mothrix");
        assert!(app.state.config_dirty);

        // Down to "Notifications" and switch it off.
        for _ in 0..3 {
            app.on_event(key(KeyCode::Down));
        }
        app.on_event(key(KeyCode::Char(' ')));
        assert!(!app.state.prefs.notifications);
        assert_eq!(app.setting_value(Setting::Notifications), "off");

        app.on_event(key(KeyCode::Esc));
        assert!(!app.overlays.is_open::<SettingsDialog>());
    }

    #[test]
    fn picture_setting_cycles_and_follows_what_the_terminal_supports() {
        let mut app = chat();
        app.pictures.kitty_supported = true;
        app.change_setting(Setting::Pictures, true);
        assert_eq!(
            (app.state.prefs.images.as_str(), app.pictures.graphics),
            ("kitty", Graphics::Kitty)
        );
        app.change_setting(Setting::Pictures, true);
        assert_eq!(
            (app.state.prefs.images.as_str(), app.pictures.graphics),
            ("sixel", Graphics::Sixel)
        );
        app.change_setting(Setting::Pictures, true);
        assert_eq!(
            (app.state.prefs.images.as_str(), app.pictures.graphics),
            ("blocks", Graphics::Blocks)
        );
        app.change_setting(Setting::Pictures, true);
        assert_eq!(app.pictures.graphics, Graphics::Off);
        app.change_setting(Setting::Pictures, true);
        assert_eq!(
            (app.state.prefs.images.as_str(), app.pictures.graphics),
            ("auto", Graphics::Kitty)
        );
    }

    #[test]
    fn a_hit_in_the_open_room_leads_out_of_its_thread() {
        let mut app = chat();
        let mut commands = Vec::new();
        app.enter_thread("$1".into(), &mut commands);
        app.on_update(Update::Thread {
            room_id: "!a".into(),
            root: "$1".into(),
            items: Vec::new(),
        });
        app.overlays.open(Search::default());
        let mut commands = Vec::new();
        app.open_hit("!a".into(), "$2".into(), &mut commands);
        assert!(matches!(
            &commands[..],
            [Command::CloseThread, Command::Reveal { event_id }] if event_id == "$2"
        ));
        assert_eq!(app.state.open_thread, None);
        // The room's conversation comes back, and the message is selected.
        app.on_update(Update::Timeline {
            room_id: "!a".into(),
            items: vec![Item {
                key: "$2".into(),
                event_id: "$2".into(),
                body: "second".into(),
                ..Item::default()
            }],
        });
        assert_eq!(app.selected.as_deref(), Some("$2"));
        assert_eq!(app.notice, None);
    }

    #[test]
    fn a_thread_that_cannot_be_opened_shows_the_room_again() {
        let mut app = chat();
        let mut commands = Vec::new();
        app.enter_thread("$1".into(), &mut commands);
        app.on_update(Update::ThreadFailed {
            room_id: "!a".into(),
            root: "$1".into(),
        });
        assert_eq!(app.state.open_thread, None);
        app.on_update(Update::Timeline {
            room_id: "!a".into(),
            items: vec![Item::default()],
        });
        assert_eq!(app.state.items.len(), 1);
    }

    #[test]
    fn search_runs_on_enter_and_enter_again_opens_the_result() {
        let mut app = chat();
        app.on_event(ctrl('f'));
        assert!(
            app.overlays
                .get::<Search>()
                .is_some_and(|search| !search.all_chats)
        );
        type_text(&mut app, "pizza");
        assert!(app.composer.is_empty(), "typing goes into the search box");

        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(
            &commands[..],
            [Command::Search { query, room_id: Some(room) }] if query == "pizza" && room == "!a"
        ));
        app.on_update(Update::SearchResults {
            query: "pizza".into(),
            hits: vec![hit("!a", "$9", "pizza one"), hit("!a", "$1", "pizza two")],
        });

        app.on_event(key(KeyCode::Down));
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::Reveal { event_id }] if event_id == "$1"));
        assert!(!app.overlays.is_open::<Search>());
        // "$1" is already loaded, so it is selected right away.
        assert_eq!(app.selected.as_deref(), Some("$1"));
    }

    #[test]
    fn searching_everywhere_opens_the_other_chat_and_waits_for_the_message() {
        let mut app = chat();
        app.on_event(ctrl('f'));
        app.on_event(key(KeyCode::Tab));
        type_text(&mut app, "deploy");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(
            &commands[..],
            [Command::Search { room_id: None, .. }]
        ));
        app.on_update(Update::SearchResults {
            query: "deploy".into(),
            hits: vec![hit("!b", "$77", "deploy now")],
        });

        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(
            &commands[..],
            [Command::Open(room), Command::Reveal { event_id }] if room == "!b" && event_id == "$77"
        ));
        assert!(app.selected.is_none());

        // The message arrives later; then it gets selected.
        app.on_update(Update::Reveal("$77".into()));
        assert!(app.selected.is_none());
        app.on_update(Update::Timeline {
            room_id: "!b".into(),
            items: vec![Item {
                key: "k".into(),
                event_id: "$77".into(),
                ..Item::default()
            }],
        });
        assert_eq!(app.selected.as_deref(), Some("$77"));
        assert!(app.viewport.wants_reveal());
    }

    #[test]
    fn results_for_an_outdated_query_are_dropped() {
        let mut app = chat();
        app.on_event(ctrl('f'));
        type_text(&mut app, "new");
        app.on_event(key(KeyCode::Enter));
        app.on_update(Update::SearchResults {
            query: "old".into(),
            hits: vec![hit("!a", "$1", "old")],
        });
        assert!(
            app.overlays
                .get::<Search>()
                .is_some_and(|search| search.hits.is_empty() && search.waiting)
        );
    }

    fn spelling() -> App {
        let mut app = chat();
        app.set_speller(
            Arc::new(Words(&[
                "hello", "there", "this", "is", "fine", "the", "then",
            ])),
            vec!["en_US".into()],
        );
        app
    }

    #[test]
    fn finished_words_are_marked_and_tab_applies_a_correction() {
        let mut app = spelling();
        type_text(&mut app, "hello teh");
        assert!(
            app.misspelled.is_empty(),
            "the word still being typed is not marked"
        );
        type_text(&mut app, " ");
        assert_eq!(app.misspelled, [(6, 9)]);
        assert!(
            app.spelling_rows().is_empty(),
            "no popup unless the cursor is on the word"
        );

        app.on_event(key(KeyCode::Left));
        let rows = app.spelling_rows();
        assert_eq!(rows[0].0, "there");
        assert!(rows.last().is_some_and(|row| row.0.starts_with("Ignore")));

        app.on_event(key(KeyCode::Down)); // starts choosing, on the first row
        app.on_event(key(KeyCode::Down));
        app.on_event(key(KeyCode::Down)); // "the"
        app.on_event(key(KeyCode::Enter));
        assert_eq!(app.composer.text(), "hello the ");
        assert!(app.misspelled.is_empty());
    }

    #[test]
    fn enter_still_sends_while_the_spelling_popup_is_open() {
        let mut app = spelling();
        type_text(&mut app, "teh ");
        app.on_event(key(KeyCode::Left));
        assert!(!app.spelling_rows().is_empty());
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::Send { text, .. }] if text == "teh"));
    }

    #[test]
    fn ignored_words_and_member_names_are_not_marked() {
        let mut app = spelling();
        type_text(&mut app, "hello Okafor ");
        assert_eq!(app.misspelled, [(6, 12)]);
        app.on_event(key(KeyCode::Left));
        let ignore_row = app.spelling_rows().len() - 1;
        app.accept_spelling(ignore_row);
        assert!(app.misspelled.is_empty());
        assert_eq!(app.take_new_ignored_words(), ["okafor"]);
        assert!(app.take_new_ignored_words().is_empty());

        app.composer.clear();
        app.on_update(Update::Members {
            room_id: "!a".into(),
            members: vec![Member {
                user_id: "@cleo:x".into(),
                name: "Cleo Marchetti".into(),
            }],
        });
        type_text(&mut app, "hello Marchetti ");
        assert!(app.misspelled.is_empty());
    }

    #[test]
    fn colon_names_complete_to_emoji() {
        let mut app = chat();
        type_text(&mut app, "nice :tad");
        let Popup { title, rows, .. } = app.popup();
        assert_eq!(title, " emoji · Tab ");
        assert_eq!(rows[0], ("🎉".to_owned(), ":tada:".to_owned()));
        app.on_event(key(KeyCode::Tab));
        assert_eq!(app.composer.text(), "nice 🎉");

        // Times and smileys do not open the popup.
        app.composer.clear();
        type_text(&mut app, "at 10:30");
        assert!(app.emoji_matches().is_empty());
        app.composer.clear();
        type_text(&mut app, "ok :)");
        assert!(app.emoji_matches().is_empty());
    }

    #[test]
    fn a_bare_colon_offers_favourites() {
        let mut app = chat();
        type_text(&mut app, "hi :");
        assert_eq!(app.emoji_matches().len(), 6);
        app.on_event(key(KeyCode::Tab));
        assert_eq!(app.composer.text(), "hi 👍");
        // Enter does not take one: "hi :" and "haha :D" are messages.
        for text in ["hi :", "haha :D"] {
            app.composer.set(text);
            let commands = app.on_event(key(KeyCode::Enter));
            assert!(
                matches!(&commands[..], [Command::Send { text: sent, .. }] if sent == text),
                "{commands:?}"
            );
        }
    }

    fn grammar(start: usize, end: usize, text: &str, replacement: &str) -> GrammarMatch {
        GrammarMatch {
            start,
            end,
            text: text.into(),
            message: "Subject and verb do not agree".into(),
            replacements: vec![replacement.into()],
            spelling: false,
        }
    }

    #[test]
    fn nothing_is_sent_without_a_languagetool_server() {
        let mut app = spelling();
        type_text(&mut app, "this are fine");
        assert!(app.take_grammar_request().is_none());
    }

    #[test]
    fn languagetool_findings_are_marked_and_can_be_applied() {
        let mut app = spelling();
        app.set_languagetool_url("http://lt.test".into());
        type_text(&mut app, "this are fine");
        assert_eq!(app.take_grammar_request().as_deref(), Some("this are fine"));
        assert!(app.take_grammar_request().is_none(), "asked once per text");
        app.set_grammar("this are fine".into(), Ok(vec![grammar(5, 8, "are", "is")]));
        assert_eq!(app.grammar_marks, [(5, 8)]);

        // Typing on keeps the finding; the new text is checked locally at once.
        type_text(&mut app, " teh ");
        assert_eq!(app.grammar_marks, [(5, 8)]);
        assert_eq!(app.misspelled, [(14, 17)]);
        assert!(app.take_grammar_request().is_some());

        app.composer.set_cursor(6);
        app.on_event(key(KeyCode::Left));
        let Popup { title, rows, .. } = app.popup();
        assert!(title.contains("Subject and verb"), "{title}");
        assert_eq!(rows[0].0, "is");
        assert_eq!(rows.len(), 1, "grammar findings have no ignore row");
        app.on_event(key(KeyCode::Tab));
        assert!(app.composer.text().starts_with("this is fine"));
    }

    #[test]
    fn commands_are_not_sent_and_stale_answers_are_dropped() {
        let mut app = spelling();
        app.set_languagetool_url("http://lt.test".into());
        type_text(&mut app, "/poll teh");
        assert!(app.take_grammar_request().is_none());

        app.composer.clear();
        type_text(&mut app, "this are");
        app.take_grammar_request();
        type_text(&mut app, " fine");
        app.take_grammar_request();
        app.set_grammar("this are".into(), Ok(vec![grammar(5, 8, "are", "is")]));
        assert!(app.grammar_marks.is_empty());
    }

    #[test]
    fn an_unreachable_server_falls_back_to_the_built_in_check() {
        let mut app = spelling();
        app.set_languagetool_url("http://lt.test".into());
        type_text(&mut app, "teh ");
        let text = app.take_grammar_request().expect("a request");
        app.set_grammar(text, Err("cannot reach http://lt.test/v2/check".into()));
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("cannot reach"))
        );
        assert_eq!(app.misspelled, [(0, 3)]);
    }

    #[test]
    fn the_languagetool_address_is_typed_in_the_settings_dialog() {
        let mut app = chat();
        app.on_event(key(KeyCode::F(2)));
        app.overlays.get_mut::<SettingsDialog>().unwrap().cursor =
            Setting::LanguageToolServer.index();
        assert_eq!(app.setting_value(Setting::LanguageToolServer), "not set");
        app.on_event(key(KeyCode::Enter));
        type_text(&mut app, "http://lt");
        app.on_event(Event::Paste(".test:8010 ".into()));
        assert!(app.composer.is_empty());
        app.on_event(key(KeyCode::Enter));
        assert_eq!(app.state.prefs.languagetool_url, "http://lt.test:8010");
        assert!(app.state.config_dirty && app.overlays.is_open::<SettingsDialog>());

        // Esc while typing only cancels the typing.
        app.on_event(key(KeyCode::Enter));
        type_text(&mut app, "xyz");
        app.on_event(key(KeyCode::Esc));
        assert_eq!(app.state.prefs.languagetool_url, "http://lt.test:8010");
        assert!(app.overlays.is_open::<SettingsDialog>());
    }

    #[test]
    fn rooms_go_into_own_sections_of_the_mixed_list() {
        let mut app = chat();
        app.focus = Focus::Sidebar;
        app.cursor_room = Some("!b".into());
        app.on_event(key(KeyCode::Char('g')));
        assert!(
            app.state.prefs.mixed_list,
            "own sections switch the mixed list on"
        );
        // Only "New section…" so far.
        app.on_event(key(KeyCode::Enter));
        type_text(&mut app, "Work");
        app.on_event(key(KeyCode::Enter));
        assert!(!app.overlays.is_open::<SectionPicker>() && app.state.config_dirty);
        assert_eq!(
            app.sidebar_rows(),
            [
                SidebarRow::Header {
                    title: "Work".into()
                },
                SidebarRow::Room { index: 1 },
                SidebarRow::Header {
                    title: "CHATS".into()
                },
                SidebarRow::Room { index: 0 },
            ]
        );

        // The other room joins the same section through the list.
        app.cursor_room = Some("!a".into());
        app.on_event(key(KeyCode::Char('g')));
        assert_eq!(app.section_picker_rows()[0], ("Work".to_owned(), false));
        app.on_event(key(KeyCode::Enter));
        assert_eq!(app.state.sections[0].rooms, ["!b", "!a"]);

        // A name that cannot be is refused, and the picker stays open.
        app.on_event(key(KeyCode::Char('g')));
        app.on_event(key(KeyCode::Down));
        app.on_event(key(KeyCode::Enter));
        type_text(&mut app, "pinned");
        app.on_event(key(KeyCode::Enter));
        assert!(app.overlays.is_open::<SectionPicker>());
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("taken"))
        );
        assert_eq!(app.state.sections.len(), 1);
    }

    #[test]
    fn own_sections_are_not_used_outside_the_mixed_list() {
        let mut app = chat();
        app.assign_section("!b", Some("Work"));
        assert_eq!(
            app.sidebar_rows()[0],
            SidebarRow::Header {
                title: "ROOMS".into()
            }
        );
    }

    #[test]
    fn long_message_folding_is_a_setting() {
        let mut app = chat();
        assert_eq!(
            app.setting_value(Setting::LongMessages),
            "folded after 15 lines"
        );
        app.change_setting(Setting::LongMessages, true);
        app.change_setting(Setting::LongMessages, true);
        assert_eq!(
            (
                app.state.prefs.long_messages,
                app.setting_value(Setting::LongMessages).as_str()
            ),
            (0, "shown in full")
        );
    }

    #[test]
    fn the_section_manager_survives_sections_going_from_elsewhere() {
        let mut app = chat();
        app.assign_section("!a", Some("Work"));
        app.assign_section("!b", Some("Family"));
        app.open_section_manager(1);
        // Another device took the second one, then the first.
        app.assign_section("!b", None);
        app.on_event(key(KeyCode::Down));
        app.on_event(key(KeyCode::Up));
        app.on_event(key(KeyCode::Char('r')));
        app.on_event(key(KeyCode::Enter));
        assert!(app.overlays.is_open::<SectionManager>());
        app.assign_section("!a", None);
        app.on_event(key(KeyCode::Down));
        assert!(!app.overlays.is_open::<SectionManager>());
    }

    #[test]
    fn sections_can_be_renamed_reordered_and_removed() {
        let mut app = chat();
        app.state.prefs.mixed_list = true;
        app.assign_section("!a", Some("Work"));
        app.assign_section("!b", Some("Family"));
        app.state.folded.toggle("Work");
        app.focus = Focus::Sidebar;
        app.on_event(key(KeyCode::Char('G')));
        assert!(app.overlays.is_open::<SectionManager>());

        // Move "Work" below "Family".
        app.on_event(key(KeyCode::Char('J')));
        let names = |app: &App| -> Vec<String> {
            app.state
                .sections
                .iter()
                .map(|section| section.name.clone())
                .collect()
        };
        assert_eq!(names(&app), ["Family", "Work"]);
        assert_eq!(
            app.overlays
                .get::<SectionManager>()
                .map(|manager| manager.cursor),
            Some(1)
        );

        // Rename it; a taken name is refused, and folding follows the name.
        app.on_event(key(KeyCode::Char('r')));
        app.on_event(ctrl('u'));
        type_text(&mut app, "family");
        app.on_event(key(KeyCode::Enter));
        assert_eq!(names(&app), ["Family", "Work"]);
        app.on_event(ctrl('u'));
        type_text(&mut app, "Job");
        app.on_event(key(KeyCode::Enter));
        assert_eq!(names(&app), ["Family", "Job"]);
        assert!(app.state.folded.contains("Job") && !app.state.folded.contains("Work"));
        assert_eq!(app.state.sections[1].rooms, ["!a"]);

        // Remove it: the chat goes back to the general list.
        app.on_event(key(KeyCode::Char('d')));
        assert_eq!(names(&app), ["Family"]);
        app.edit_section(SectionEdit::Delete(0));
        assert!(!app.overlays.is_open::<SectionManager>() && app.state.sections.is_empty());
    }

    #[test]
    fn languagetool_languages_are_typed_in_the_settings_dialog() {
        let mut app = spelling();
        app.on_event(key(KeyCode::F(2)));
        app.overlays.get_mut::<SettingsDialog>().unwrap().cursor =
            Setting::LanguageToolLanguages.index();
        assert_eq!(
            app.setting_value(Setting::LanguageToolLanguages),
            "any (detected)"
        );
        app.on_event(key(KeyCode::Enter));
        type_text(&mut app, "de, en");
        app.on_event(key(KeyCode::Enter));
        assert_eq!(app.state.prefs.languagetool_languages, ["de-DE", "en-US"]);
        assert_eq!(
            app.setting_value(Setting::LanguageToolLanguages),
            "de-DE, en-US"
        );
        assert!(app.state.config_dirty);
        // Emptying it goes back to detecting.
        app.on_event(key(KeyCode::Enter));
        app.on_event(ctrl('u'));
        app.on_event(key(KeyCode::Enter));
        assert!(app.state.prefs.languagetool_languages.is_empty());
    }

    #[test]
    fn enter_applies_a_correction_once_one_is_chosen() {
        let mut app = spelling();
        type_text(&mut app, "teh ");
        app.on_event(key(KeyCode::Left));
        assert_eq!(app.popup().selected, None, "nothing is highlighted yet");
        // Down chooses the first correction; Enter then applies it
        // instead of sending.
        app.on_event(key(KeyCode::Down));
        assert_eq!(app.popup().selected, Some(0));
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(commands.is_empty());
        assert_eq!(app.composer.text(), "there ");
        // With nothing left to correct, Enter sends again.
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(matches!(&commands[..], [Command::Send { text, .. }] if text == "there"));
    }

    #[test]
    fn the_paid_service_account_is_set_in_the_dialog_and_the_key_stays_hidden() {
        let mut app = chat();
        app.on_event(key(KeyCode::F(2)));
        app.overlays.get_mut::<SettingsDialog>().unwrap().cursor = Setting::LanguageToolKey.index();
        app.on_event(key(KeyCode::Enter));
        type_text(&mut app, "secret-key");
        app.on_event(key(KeyCode::Enter));
        assert_eq!(app.state.prefs.languagetool_api_key, "secret-key");
        assert_eq!(app.setting_value(Setting::LanguageToolKey), "set (hidden)");
        app.on_event(key(KeyCode::Up));
        app.on_event(key(KeyCode::Enter));
        type_text(&mut app, "me@example.org");
        app.on_event(key(KeyCode::Enter));
        assert_eq!(
            app.setting_value(Setting::LanguageToolUser),
            "me@example.org"
        );
    }

    #[test]
    fn spell_check_can_be_switched_off() {
        let mut app = spelling();
        type_text(&mut app, "teh ");
        assert!(!app.misspelled.is_empty());
        app.change_setting(Setting::SpellCheck, true);
        assert!(app.misspelled.is_empty());
        assert_eq!(app.setting_value(Setting::SpellCheck), "off");
    }

    #[test]
    fn join_and_leave_messages_can_be_hidden() {
        let mut app = chat();
        assert_eq!(app.setting_value(Setting::RoomEvents), "shown");
        app.change_setting(Setting::RoomEvents, true);
        assert!(!app.state.prefs.room_events);
    }
}
