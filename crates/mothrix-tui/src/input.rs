// SPDX-License-Identifier: MPL-2.0

//! A small single-buffer line editor: the message box and every field a
//! dialog types into. [`Input::edit`] takes the editing keys and
//! [`Input::view`] says what of the text to draw and where the cursor goes,
//! so fields edit and look the same everywhere.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Debug, Default)]
pub struct Input {
    text: String,
    /// Cursor position as a char index (not bytes).
    cursor: usize,
}

impl Input {
    /// Take an editing key: typing, Backspace, Delete, ←, →, Home, End,
    /// Ctrl+U (clear) and Ctrl+W (the word before the cursor). False for
    /// any other key, which is the caller's (Enter, Esc, ↑, ↓, …).
    pub fn edit(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Home => self.home(),
            KeyCode::End => self.end(),
            KeyCode::Char('u') if ctrl => self.clear(),
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Char(c) if !ctrl && !alt => self.insert(c),
            _ => return false,
        }
        true
    }

    /// What of the text fits into `width` terminal cells with the cursor in
    /// view, and the cell the cursor is in (counted from the left of it).
    pub fn view(&self, width: usize) -> (String, u16) {
        let chars: Vec<char> = self.text.chars().collect();
        let cursor = self.cursor.min(chars.len());
        let cells = |c: &char| crate::inline::cells(*c);
        // Start as far left as the cursor (and a cell for it) allows.
        let mut skip = cursor;
        let mut used = 1;
        while skip > 0 && used + cells(&chars[skip - 1]) <= width {
            skip -= 1;
            used += cells(&chars[skip]);
        }
        let mut shown = String::new();
        let mut filled = 0;
        for c in &chars[skip..] {
            if filled + cells(c) > width {
                break;
            }
            filled += cells(c);
            shown.push(*c);
        }
        let column: usize = chars[skip..cursor].iter().map(cells).sum();
        (shown, column as u16)
    }

    /// The same with every character shown as `•` (a password).
    pub fn masked(&self) -> Self {
        Self {
            text: self.text.chars().map(|_| '•').collect(),
            cursor: self.cursor,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.text.len())
    }

    fn len_chars(&self) -> usize {
        self.text.chars().count()
    }

    pub fn insert(&mut self, c: char) {
        let at = self.byte_index(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars().filter(|c| *c != '\r') {
            self.insert(c);
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_index(self.cursor - 1);
        let end = self.byte_index(self.cursor);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.len_chars() {
            return;
        }
        let start = self.byte_index(self.cursor);
        let end = self.byte_index(self.cursor + 1);
        self.text.replace_range(start..end, "");
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.len_chars());
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.len_chars();
    }

    /// Delete the word before the cursor (Ctrl+W).
    pub fn delete_word(&mut self) {
        let chars: Vec<char> = self.text.chars().collect();
        self.cursor = self.cursor.min(chars.len());
        let mut start = self.cursor;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        let from = self.byte_index(start);
        let to = self.byte_index(self.cursor);
        self.text.replace_range(from..to, "");
        self.cursor = start;
    }

    /// Replace the chars in `start..end` with `with` and put the cursor
    /// right after the inserted text. A range past the end of the text
    /// (one made for a text that was replaced since) ends with the text:
    /// the cursor is never left beyond it.
    pub fn replace_range(&mut self, start: usize, end: usize, with: &str) {
        let start = start.min(self.len_chars());
        let from = self.byte_index(start);
        let to = self.byte_index(end.max(start));
        self.text.replace_range(from..to, with);
        self.cursor = start + with.chars().count();
    }

    /// Move the cursor to the given char index (clamped to the text).
    pub fn set_cursor(&mut self, index: usize) {
        self.cursor = index.min(self.len_chars());
    }

    /// Move the cursor to the line above, in text with line breaks.
    /// False (and nothing moves) on the first line.
    pub fn line_up(&mut self) -> bool {
        let chars: Vec<char> = self.text().chars().collect();
        let cursor = self.cursor().min(chars.len());
        let start_of = |end: usize| {
            chars[..end]
                .iter()
                .rposition(|c| *c == '\n')
                .map_or(0, |at| at + 1)
        };
        let line_start = start_of(cursor);
        if line_start == 0 {
            return false;
        }
        let above_end = line_start - 1;
        let above_start = start_of(above_end);
        self.set_cursor(above_start + (cursor - line_start).min(above_end - above_start));
        true
    }

    /// Move the cursor to the line below. False on the last line.
    pub fn line_down(&mut self) -> bool {
        let chars: Vec<char> = self.text().chars().collect();
        let cursor = self.cursor().min(chars.len());
        let Some(to_break) = chars[cursor..].iter().position(|c| *c == '\n') else {
            return false;
        };
        let line_start = chars[..cursor]
            .iter()
            .rposition(|c| *c == '\n')
            .map_or(0, |at| at + 1);
        let below_start = cursor + to_break + 1;
        let below_end = chars[below_start..]
            .iter()
            .position(|c| *c == '\n')
            .map_or(chars.len(), |at| below_start + at);
        self.set_cursor(below_start + (cursor - line_start).min(below_end - below_start));
        true
    }

    /// The text before the cursor.
    pub fn before_cursor(&self) -> &str {
        &self.text[..self.byte_index(self.cursor)]
    }

    /// Replace the text and put the cursor at its end.
    pub fn set(&mut self, text: &str) {
        self.text = text.to_owned();
        self.cursor = self.len_chars();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Take the text out, leaving the input empty.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn editing_keys_edit_and_others_are_left_to_the_caller() {
        let mut input = Input::default();
        for c in "hello world".chars() {
            assert!(input.edit(key(KeyCode::Char(c), KeyModifiers::NONE)));
        }
        assert!(input.edit(key(KeyCode::Char('w'), KeyModifiers::CONTROL)));
        assert_eq!(input.text(), "hello ");
        assert!(input.edit(key(KeyCode::Left, KeyModifiers::NONE)));
        assert!(input.edit(key(KeyCode::Backspace, KeyModifiers::NONE)));
        assert_eq!(input.text(), "hell ");
        assert!(!input.edit(key(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(!input.edit(key(KeyCode::Up, KeyModifiers::NONE)));
        assert!(!input.edit(key(KeyCode::Char('x'), KeyModifiers::ALT)));
        assert!(input.edit(key(KeyCode::Char('u'), KeyModifiers::CONTROL)));
        assert!(input.is_empty());
    }

    #[test]
    fn the_view_keeps_the_cursor_in_sight_and_counts_wide_characters() {
        let mut input = Input::default();
        input.set("abcdefghij");
        // The end shows, with a cell left for the cursor.
        assert_eq!(input.view(5), ("ghij".to_owned(), 4));
        input.home();
        assert_eq!(input.view(5), ("abcde".to_owned(), 0));
        // Wide characters take two cells each.
        input.set("日本語です");
        assert_eq!(input.view(7), ("語です".to_owned(), 6));
        input.set_cursor(1);
        assert_eq!(input.view(20), ("日本語です".to_owned(), 2));
        assert_eq!(input.masked().view(20), ("•••••".to_owned(), 1));
    }

    #[test]
    fn edits_respect_multibyte_characters() {
        let mut input = Input::default();
        input.insert_str("héllo wörld");
        assert_eq!(input.cursor(), 11);
        input.backspace();
        assert_eq!(input.text(), "héllo wörl");
        input.home();
        input.right();
        input.right();
        input.backspace();
        assert_eq!(input.text(), "hllo wörl");
        input.delete();
        assert_eq!(input.text(), "hlo wörl");
    }

    #[test]
    fn delete_word_removes_the_previous_word_and_spaces() {
        let mut input = Input::default();
        input.insert_str("one two  ");
        input.delete_word();
        assert_eq!(input.text(), "one ");
        input.delete_word();
        assert_eq!(input.text(), "");
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn a_range_past_the_end_leaves_the_cursor_inside_the_text() {
        // A range made for a longer text that was replaced since.
        let mut input = Input::default();
        input.set("ab");
        input.replace_range(6, 9, "there");
        assert_eq!(input.text(), "abthere");
        assert_eq!(input.cursor(), 7);
        // Every key after it works on the text that is there.
        input.delete_word();
        assert_eq!(input.text(), "");
        input.set("ab");
        input.replace_range(6, 9, "");
        input.insert('c');
        input.backspace();
        input.delete();
        input.line_up();
        input.line_down();
        assert_eq!((input.text(), input.cursor()), ("ab", 2));
        assert_eq!(input.before_cursor(), "ab");
    }

    #[test]
    fn take_empties_the_input() {
        let mut input = Input::default();
        input.insert_str("hi");
        assert_eq!(input.take(), "hi");
        assert!(input.is_empty());
        assert_eq!(input.cursor(), 0);
    }
}
