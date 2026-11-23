// SPDX-License-Identifier: MPL-2.0

//! The emoji picker: every emoji by group under the ones the user takes
//! most, a search line above them, a grid to move in. It reacts to a
//! message (opened from the reaction bar) or puts the emoji into the
//! message box (the `+ emoji` button, `F5`).

use std::sync::OnceLock;

use super::*;
use crate::emoji::{self, Group};

/// Terminal columns per emoji in the grid (an emoji is two wide).
pub const CELL_WIDTH: u16 = 4;

/// What the picked emoji is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickFor {
    /// A reaction to the message with this event id.
    React(String),
    /// Into the message box, at the cursor.
    Insert,
}

/// The emoji picker dialog.
#[derive(Clone, Debug)]
pub struct EmojiPicker {
    pub target: PickFor,
    pub search: Input,
    /// The emoji under the cursor: an index into [`PickerGrid::emoji`].
    pub cursor: usize,
    /// The first row of the grid in view.
    pub scroll: usize,
    /// Emoji per row and rows in view, as last drawn.
    pub columns: usize,
    pub page: usize,
}

impl EmojiPicker {
    pub fn new(target: PickFor) -> Self {
        Self {
            target,
            search: Input::default(),
            cursor: 0,
            scroll: 0,
            columns: 10,
            page: 10,
        }
    }

    /// The search changed: start again at the top.
    pub(crate) fn searched(&mut self) {
        self.cursor = 0;
        self.scroll = 0;
    }
}

/// One row of the grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GridRow {
    Title(&'static str),
    /// The emoji `start..end` of [`PickerGrid::emoji`].
    Emoji {
        start: usize,
        end: usize,
    },
}

/// What the picker shows, laid out in rows of its width.
#[derive(Clone, Debug, Default)]
pub struct PickerGrid {
    /// Every emoji shown, in order: (emoji, the words it is found by).
    pub emoji: Vec<(String, &'static str)>,
    pub rows: Vec<GridRow>,
}

/// Title of the user's favourites in the picker.
pub const USED_OFTEN: &str = "Used often";

/// Every emoji by group, worked out once.
fn all_groups() -> &'static [Group] {
    static GROUPS: OnceLock<Vec<Group>> = OnceLock::new();
    GROUPS.get_or_init(emoji::groups)
}

/// The words an emoji is found by, if it is one of [`all_groups`].
fn words_of(emoji: &str) -> &'static str {
    let bare = emoji.trim_end_matches('\u{fe0f}');
    all_groups()
        .iter()
        .flat_map(|group| group.emoji.iter())
        .find(|(known, _)| *known == emoji || known.trim_end_matches('\u{fe0f}') == bare)
        .map(|(_, words)| words.as_str())
        .unwrap_or("")
}

/// How well an emoji's `words` fit the typed `query` (lower case): 0 when
/// every term is one of the words, 1 when every term at least starts one,
/// 2 when some term is only somewhere inside; `None` when a term is not
/// there at all.
pub fn rank(words: &str, query: &str) -> Option<u8> {
    let words = words.to_lowercase();
    let pieces: Vec<&str> = words
        .split(|c: char| !c.is_alphanumeric() && c != '+')
        .filter(|piece| !piece.is_empty())
        .collect();
    let mut worst = None;
    for term in query.split_whitespace() {
        let fit = if pieces.contains(&term) {
            0
        } else if pieces.iter().any(|piece| piece.starts_with(term)) {
            1
        } else if words.contains(term) {
            2
        } else {
            return None;
        };
        worst = Some(worst.map_or(fit, |known: u8| known.max(fit)));
    }
    worst
}

/// The emoji of `groups` whose words fit `query`, the best fits first;
/// among equals, `favourites` first, then the usual order.
pub fn search<'a>(
    groups: &'a [Group],
    query: &str,
    favourites: &[String],
) -> Vec<(&'static str, &'a str)> {
    let query = query.trim().to_lowercase();
    let mut found = Vec::new();
    for group in groups {
        for (emoji, words) in &group.emoji {
            if let Some(fit) = rank(words, &query) {
                let favourite = favourites
                    .iter()
                    .position(|favourite| favourite == emoji)
                    .unwrap_or(usize::MAX);
                found.push((fit, favourite, *emoji, words.as_str()));
            }
        }
    }
    found.sort_by_key(|(fit, favourite, _, _)| (*fit, *favourite));
    found
        .into_iter()
        .map(|(_, _, emoji, words)| (emoji, words))
        .collect()
}

impl PickerGrid {
    /// The grid for a search (empty: everything, favourites first) at
    /// `columns` emoji per row.
    pub fn new(query: &str, favourites: &[String], columns: usize) -> Self {
        let columns = columns.max(1);
        let mut grid = Self::default();
        if query.trim().is_empty() {
            grid.add(
                USED_OFTEN,
                favourites
                    .iter()
                    .map(|emoji| (emoji.clone(), words_of(emoji)))
                    .collect(),
                columns,
            );
            for group in all_groups() {
                grid.add(
                    group.title,
                    group
                        .emoji
                        .iter()
                        .map(|(emoji, words)| ((*emoji).to_owned(), words.as_str()))
                        .collect(),
                    columns,
                );
            }
        } else {
            let found: Vec<(String, &'static str)> = search(all_groups(), query, favourites)
                .into_iter()
                .map(|(emoji, words)| (emoji.to_owned(), words))
                .collect();
            for chunk in found.chunks(columns) {
                grid.push_row(chunk.to_vec());
            }
        }
        grid
    }

    fn add(&mut self, title: &'static str, emoji: Vec<(String, &'static str)>, columns: usize) {
        if emoji.is_empty() {
            return;
        }
        self.rows.push(GridRow::Title(title));
        for chunk in emoji.chunks(columns) {
            self.push_row(chunk.to_vec());
        }
    }

    fn push_row(&mut self, emoji: Vec<(String, &'static str)>) {
        let start = self.emoji.len();
        self.emoji.extend(emoji);
        self.rows.push(GridRow::Emoji {
            start,
            end: self.emoji.len(),
        });
    }

    /// The row the emoji at `index` is in.
    pub fn row_of(&self, index: usize) -> Option<usize> {
        self.rows.iter().position(
            |row| matches!(row, GridRow::Emoji { start, end } if (*start..*end).contains(&index)),
        )
    }

    /// The emoji `rows` rows of emoji further down (up when negative), in
    /// the same column as far as that row goes.
    fn vertical(&self, index: usize, rows: isize) -> usize {
        let emoji_rows: Vec<(usize, usize)> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                GridRow::Emoji { start, end } => Some((*start, *end)),
                GridRow::Title(_) => None,
            })
            .collect();
        let Some(at) = emoji_rows
            .iter()
            .position(|(start, end)| (*start..*end).contains(&index))
        else {
            return 0;
        };
        let column = index - emoji_rows[at].0;
        let target = (at as isize + rows).clamp(0, emoji_rows.len() as isize - 1) as usize;
        let (start, end) = emoji_rows[target];
        (start + column).min(end - 1)
    }
}

impl App {
    /// Open the emoji picker.
    pub(crate) fn open_emoji_picker(&mut self, target: PickFor) {
        if target == PickFor::Insert && self.state.open_room.is_none() {
            self.notice = Some("Open a room first.".to_owned());
            return;
        }
        self.overlays.open(EmojiPicker::new(target));
    }

    /// What the open emoji picker shows.
    pub fn emoji_grid(&self) -> PickerGrid {
        let Some(picker) = self.overlays.get::<EmojiPicker>() else {
            return PickerGrid::default();
        };
        let favourites = self.emoji_usage.favourites(picker.columns.max(1) * 2);
        PickerGrid::new(picker.search.text(), &favourites, picker.columns)
    }

    /// Take the emoji at `index` of the picker's grid: react with it or
    /// put it into the message box. Closes the picker.
    pub(crate) fn pick_emoji(&mut self, emoji: String, commands: &mut Vec<Command>) {
        let Some(picker) = self.overlays.take::<EmojiPicker>() else {
            return;
        };
        match picker.target {
            // The same way as the reaction bar, which then steps back to
            // the message's actions. (`on_event` notes the reaction.)
            PickFor::React(event_id) if self.selected.as_deref() == Some(event_id.as_str()) => {
                self.on_bar(BarAction::Emoji(emoji), commands);
            }
            PickFor::React(event_id) => commands.push(Command::React {
                event_id,
                key: emoji,
            }),
            PickFor::Insert => {
                self.composer.insert_str(&emoji);
                self.focus = Focus::Composer;
                self.used_emoji(emoji);
            }
        }
    }

    /// Scroll the picker's grid with the wheel; the next draw keeps it
    /// within the grid.
    pub(crate) fn scroll_emoji_picker(&mut self, rows: isize) {
        if let Some(picker) = self.overlays.get_mut::<EmojiPicker>() {
            picker.scroll = picker.scroll.saturating_add_signed(rows);
        }
    }

    pub(super) fn on_emoji_picker_key(&mut self, key: KeyEvent, commands: &mut Vec<Command>) {
        let grid = self.emoji_grid();
        let Some(picker) = self.overlays.get_mut::<EmojiPicker>() else {
            return;
        };
        let last = grid.emoji.len().saturating_sub(1);
        let page = picker.page.saturating_sub(1).max(1) as isize;
        let moved = match key.code {
            KeyCode::Esc => return self.overlays.close::<EmojiPicker>(),
            KeyCode::Enter => {
                if let Some((emoji, _)) = grid.emoji.get(picker.cursor).cloned() {
                    self.pick_emoji(emoji, commands);
                }
                return;
            }
            KeyCode::Left => picker.cursor.saturating_sub(1),
            KeyCode::Right => (picker.cursor + 1).min(last),
            KeyCode::Up => grid.vertical(picker.cursor, -1),
            KeyCode::Down => grid.vertical(picker.cursor, 1),
            KeyCode::PageUp => grid.vertical(picker.cursor, -page),
            KeyCode::PageDown => grid.vertical(picker.cursor, page),
            KeyCode::Home => 0,
            KeyCode::End => last,
            _ => {
                if picker.search.edit(key) {
                    picker.searched();
                }
                return;
            }
        };
        picker.cursor = moved;
        reveal(picker, &grid);
    }
}

/// Scroll so that the row of the cursor is in view (with its group's
/// title, when it is the group's first row).
pub fn reveal(picker: &mut EmojiPicker, grid: &PickerGrid) {
    let Some(row) = grid.row_of(picker.cursor) else {
        picker.scroll = 0;
        return;
    };
    let page = picker.page.max(1);
    let titled = row > 0 && matches!(grid.rows[row - 1], GridRow::Title(_));
    let top = if titled { row - 1 } else { row };
    if top < picker.scroll {
        picker.scroll = top;
    } else if row >= picker.scroll + page {
        picker.scroll = row + 1 - page;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Item;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_event(key(KeyCode::Char(c)));
        }
    }

    /// Room `!a` open with two messages.
    fn chat() -> App {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.on_update(Update::Rooms(vec![RoomEntry {
            id: "!a".into(),
            name: "Lunch Club".into(),
            ..RoomEntry::default()
        }]));
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

    fn picker(app: &App) -> &EmojiPicker {
        app.overlays.get::<EmojiPicker>().expect("picker open")
    }

    fn under_cursor(app: &App) -> String {
        app.emoji_grid().emoji[picker(app).cursor].0.clone()
    }

    #[test]
    fn whole_words_rank_before_starts_before_insides() {
        assert_eq!(rank("cat face cat", "cat"), Some(0));
        assert_eq!(rank("category", "cat"), Some(1));
        assert_eq!(rank("bobcat", "cat"), Some(2));
        assert_eq!(rank("dog face", "cat"), None);
        // Every term has to be there; the worst one counts.
        assert_eq!(rank("red heart heart", "red hea"), Some(1));
        assert_eq!(rank("red heart heart", "red cat"), None);
        // Short names with underscores are split into words.
        assert_eq!(rank("smiling cat smiley_cat", "smiley"), Some(0));

        let groups = vec![Group {
            title: "Test",
            emoji: vec![
                ("a", "bobcat".to_owned()),
                ("b", "category thing".to_owned()),
                ("c", "dog".to_owned()),
                ("d", "cat face".to_owned()),
                ("e", "cat".to_owned()),
            ],
        }];
        let order = |favourites: &[String]| -> Vec<String> {
            search(&groups, "Cat", favourites)
                .into_iter()
                .map(|(emoji, _)| emoji.to_owned())
                .collect()
        };
        assert_eq!(order(&[]), ["d", "e", "b", "a"]);
        // Among equals, what the user takes comes first.
        assert_eq!(order(&["e".to_owned()]), ["e", "d", "b", "a"]);
    }

    #[test]
    fn the_whole_grid_starts_with_favourites_then_every_group() {
        let favourites = vec!["🎉".to_owned(), "👍".to_owned(), "🦋".to_owned()];
        let grid = PickerGrid::new("", &favourites, 2);
        assert_eq!(grid.rows[0], GridRow::Title(USED_OFTEN));
        assert_eq!(grid.rows[1], GridRow::Emoji { start: 0, end: 2 });
        assert_eq!(grid.rows[2], GridRow::Emoji { start: 2, end: 3 });
        assert_eq!(grid.rows[3], GridRow::Title("Smileys"));
        assert_eq!(grid.emoji[0].0, "🎉");
        assert!(grid.emoji[0].1.contains("tada"), "{:?}", grid.emoji[0]);
        let titles = grid
            .rows
            .iter()
            .filter(|row| matches!(row, GridRow::Title(_)))
            .count();
        assert_eq!(titles, 10);
        assert!(grid.emoji.len() > 1500 + favourites.len());
        assert!(grid.emoji.iter().any(|(emoji, _)| emoji == "🇩🇪"));

        let found = PickerGrid::new("tada", &favourites, 8);
        assert_eq!(found.emoji[0].0, "🎉");
        assert!(
            !found
                .rows
                .iter()
                .any(|row| matches!(row, GridRow::Title(_)))
        );
        assert!(PickerGrid::new("zzzqqq", &favourites, 8).emoji.is_empty());
    }

    #[test]
    fn arrows_move_in_the_grid_over_the_titles() {
        let mut app = chat();
        app.on_event(key(KeyCode::F(5)));
        app.overlays.get_mut::<EmojiPicker>().unwrap().page = 4;
        assert_eq!(picker(&app).cursor, 0);
        app.on_event(key(KeyCode::Right));
        assert_eq!(picker(&app).cursor, 1);
        app.on_event(key(KeyCode::Left));
        app.on_event(key(KeyCode::Left));
        assert_eq!(picker(&app).cursor, 0);
        // Ten columns, twelve favourites: two rows, then the smileys.
        app.on_event(key(KeyCode::Down));
        assert_eq!(picker(&app).cursor, 10);
        app.on_event(key(KeyCode::Down));
        assert_eq!(picker(&app).cursor, 12);
        assert_eq!(under_cursor(&app), "😀");
        // Its row (the fifth, after "Smileys") comes into view.
        assert_eq!(picker(&app).scroll, 1);
        // A page is the rows in view but one.
        app.on_event(key(KeyCode::PageDown));
        assert_eq!(picker(&app).cursor, 42);
        app.on_event(key(KeyCode::Up));
        assert_eq!(picker(&app).cursor, 32);
        app.on_event(key(KeyCode::End));
        let last = app.emoji_grid().emoji.len() - 1;
        assert_eq!(picker(&app).cursor, last);
        app.on_event(key(KeyCode::Home));
        assert_eq!(picker(&app).cursor, 0);
        assert_eq!(picker(&app).scroll, 0);
        app.on_event(key(KeyCode::Esc));
        assert!(!app.overlays.is_open::<EmojiPicker>());
    }

    #[test]
    fn typing_searches_and_enter_puts_the_emoji_into_the_message_box() {
        let mut app = chat();
        type_text(&mut app, "nice ");
        app.take_effects();
        app.on_event(key(KeyCode::F(5)));
        type_text(&mut app, "tada");
        assert_eq!(picker(&app).search.text(), "tada");
        assert_eq!(under_cursor(&app), "🎉");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(commands.is_empty(), "{commands:?}");
        assert!(app.overlays.is_empty());
        assert_eq!(app.composer.text(), "nice 🎉");
        assert_eq!(app.focus, Focus::Composer);
        assert!(
            app.take_effects()
                .iter()
                .any(|effect| matches!(effect, Effect::UsedEmoji(emoji) if emoji == "🎉"))
        );
        assert_eq!(app.quick_reactions()[0], "🎉");
    }

    #[test]
    fn more_on_the_reaction_bar_opens_the_picker_and_picking_reacts() {
        let mut app = chat();
        app.on_event(key(KeyCode::Up));
        assert_eq!(app.selected.as_deref(), Some("$2"));
        app.on_event(key(KeyCode::Char('a')));
        assert_eq!(app.bar, Bar::Emoji);
        app.on_event(key(KeyCode::Char('0')));
        assert_eq!(picker(&app).target, PickFor::React("$2".into()));
        // Typed letters go to the search, not to the message's actions.
        type_text(&mut app, "butterfly");
        app.take_effects();
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::React { event_id, key }] if event_id == "$2" && key == "🦋"),
            "{commands:?}"
        );
        assert_eq!(app.bar, Bar::Actions);
        assert!(app.overlays.is_empty());
        assert!(
            app.take_effects()
                .iter()
                .any(|effect| matches!(effect, Effect::UsedEmoji(emoji) if emoji == "🦋"))
        );
        assert!(app.quick_reactions().contains(&"🦋".to_owned()));
    }

    #[test]
    fn pasted_text_searches_and_the_wheel_scrolls() {
        let mut app = chat();
        app.on_event(key(KeyCode::F(5)));
        app.on_event(Event::Paste("pizza".into()));
        assert_eq!(under_cursor(&app), "🍕");
        app.on_event(key(KeyCode::Esc));

        app.on_event(key(KeyCode::F(5)));
        crate::ui::render_to_text(&mut app, 100, 30);
        app.on_event(Event::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 50,
            row: 15,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(picker(&app).scroll, 3);
        assert_eq!(picker(&app).cursor, 0);
    }

    #[test]
    fn without_a_room_there_is_nothing_to_put_an_emoji_into() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.on_event(key(KeyCode::F(5)));
        assert!(app.overlays.is_empty());
        assert!(app.notice.is_some());
    }
}
