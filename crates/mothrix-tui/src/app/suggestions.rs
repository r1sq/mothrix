// SPDX-License-Identifier: MPL-2.0

//! The popup above the message box: commands after `/`, files after
//! `/file `, people after `@`, emoji after `:`, else corrections of the
//! word under the cursor. [`App::suggestions`] decides which one is up, in
//! one place; drawing, keys and choosing a row all go by that.

use super::*;

/// What the popup offers, of one kind at a time.
enum Suggestions {
    /// (name, what it does, takes an argument).
    Commands(Vec<(&'static str, &'static str, bool)>),
    /// (name shown, the path it completes to).
    Paths(Vec<(String, String)>),
    Mentions(Vec<Member>),
    /// (emoji, short name).
    Emoji(Vec<(String, String)>),
    /// Corrections of the marked word, if any (see `dialogs.rs`).
    Spelling,
}

/// The popup as drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Popup {
    pub title: String,
    /// (main text, detail); none means no popup.
    pub rows: Vec<(String, String)>,
    /// The highlighted row: corrections are not highlighted until the
    /// arrow keys choose one.
    pub selected: Option<usize>,
}

impl App {
    /// Which list the popup shows for the text as it is now.
    fn suggestions(&self) -> Suggestions {
        let commands = self.command_matches();
        if !commands.is_empty() {
            return Suggestions::Commands(commands);
        }
        let paths = self.path_matches();
        if !paths.is_empty() {
            return Suggestions::Paths(paths);
        }
        let members = self.mention_matches();
        if !members.is_empty() {
            return Suggestions::Mentions(members);
        }
        let emoji = self.emoji_matches();
        if !emoji.is_empty() {
            return Suggestions::Emoji(emoji);
        }
        Suggestions::Spelling
    }

    /// What the popup above the message box shows.
    pub fn popup(&self) -> Popup {
        let suggestions = self.suggestions();
        let offered_only = matches!(suggestions, Suggestions::Spelling) && !self.spelling_engaged();
        let (title, rows) = Self::rows_of(suggestions, self);
        let selected =
            (!rows.is_empty() && !offered_only).then(|| self.mention_selected.min(rows.len() - 1));
        Popup {
            title,
            rows,
            selected,
        }
    }

    fn rows_of(suggestions: Suggestions, app: &App) -> (String, Vec<(String, String)>) {
        match suggestions {
            Suggestions::Commands(commands) => (
                " commands · Tab ".to_owned(),
                commands
                    .into_iter()
                    .map(|(name, about, _)| (format!("/{name}"), about.to_owned()))
                    .collect(),
            ),
            Suggestions::Paths(paths) => (
                " files · Tab ".to_owned(),
                paths
                    .into_iter()
                    .map(|(name, _)| (name, String::new()))
                    .collect(),
            ),
            Suggestions::Mentions(members) => (
                " mention · Tab ".to_owned(),
                members
                    .into_iter()
                    .map(|member| (member.name, short_id(&member.user_id, 34)))
                    .collect(),
            ),
            Suggestions::Emoji(emoji) => (
                " emoji · Tab ".to_owned(),
                emoji
                    .into_iter()
                    .map(|(emoji, code)| (emoji, format!(":{code}:")))
                    .collect(),
            ),
            Suggestions::Spelling => (app.spelling_title(), app.spelling_rows()),
        }
    }

    /// Keys for the popup. Returns whether the key was used.
    pub(super) fn on_mention_key(&mut self, key: KeyEvent) -> bool {
        let suggestions = self.suggestions();
        let spelling = matches!(suggestions, Suggestions::Spelling);
        let commands = matches!(suggestions, Suggestions::Commands(_));
        let count = match &suggestions {
            Suggestions::Commands(rows) => rows.len(),
            Suggestions::Paths(rows) => rows.len(),
            Suggestions::Mentions(rows) => rows.len(),
            Suggestions::Emoji(rows) => rows.len(),
            Suggestions::Spelling => self.spelling_rows().len(),
        };
        if count == 0 {
            return false;
        }
        // Corrections are only an offer, and the popup opens by itself:
        // Enter keeps sending the message until a correction is chosen
        // with the arrow keys. Tab applies one right away.
        let offered_only = spelling && !self.spelling_engaged();
        let index = self.mention_selected.min(count - 1);
        match key.code {
            KeyCode::Up | KeyCode::Down if offered_only => {
                self.engage_spelling();
                self.mention_selected = if key.code == KeyCode::Down {
                    0
                } else {
                    count - 1
                };
            }
            KeyCode::Up => {
                self.mention_selected = self.mention_selected.checked_sub(1).unwrap_or(count - 1)
            }
            KeyCode::Down => self.mention_selected = (self.mention_selected + 1) % count,
            KeyCode::Enter if offered_only => return false,
            // Enter on a command: complete it, and run it right away if
            // it needs nothing more.
            KeyCode::Tab | KeyCode::Enter if commands => {
                let more_to_type = self.accept(suggestions, index);
                return more_to_type || key.code == KeyCode::Tab;
            }
            // ":D" or a lone ":" at the end of a message is no emoji being
            // looked for: Enter sends, Tab still takes the suggestion.
            KeyCode::Enter
                if self
                    .emoji_query()
                    .is_some_and(|(_, query)| query.chars().count() < 2) =>
            {
                return false;
            }
            KeyCode::Tab | KeyCode::Enter => {
                self.accept(suggestions, index);
            }
            KeyCode::Esc => self.mention_hidden = true,
            _ => return false,
        }
        true
    }

    /// Take the popup's row `index` (a click on it).
    pub(super) fn accept_suggestion(&mut self, index: usize) {
        let suggestions = self.suggestions();
        self.accept(suggestions, index);
    }

    /// Put row `index` of `suggestions` into the message box. For a
    /// command: true if it takes an argument (more to type).
    fn accept(&mut self, suggestions: Suggestions, index: usize) -> bool {
        match suggestions {
            Suggestions::Commands(commands) => {
                let Some((name, _, takes_argument)) = commands.get(index).copied() else {
                    return true;
                };
                self.composer.set(&if takes_argument {
                    format!("/{name} ")
                } else {
                    format!("/{name}")
                });
                self.mention_selected = 0;
                return takes_argument;
            }
            Suggestions::Paths(paths) => {
                if let Some((_, completed)) = paths.get(index) {
                    self.composer.set(&format!("/file {completed}"));
                    self.mention_selected = 0;
                }
            }
            Suggestions::Mentions(members) => {
                if let (Some(member), Some((start, _))) = (members.get(index), self.mention_query())
                {
                    let end = self.composer.cursor();
                    self.composer
                        .replace_range(start, end, &format!("@{} ", member.name));
                    if !self.composer_mentions.contains(member) {
                        self.composer_mentions.push(member.clone());
                    }
                    self.mention_selected = 0;
                }
            }
            Suggestions::Emoji(emoji) => {
                if let (Some((emoji, _)), Some((start, _))) = (emoji.get(index), self.emoji_query())
                {
                    let end = self.composer.cursor();
                    let emoji = emoji.clone();
                    self.composer.replace_range(start, end, &emoji);
                    self.mention_selected = 0;
                    self.used_emoji(emoji);
                }
            }
            Suggestions::Spelling => self.accept_spelling(index),
        }
        false
    }

    // -- @-mentions ---------------------------------------------------------

    /// The `@word` being typed right before the cursor: (char index of the
    /// `@`, the text after it). Only at the start or after whitespace, so
    /// e-mail addresses do not trigger it.
    fn mention_query(&self) -> Option<(usize, String)> {
        let before = self.composer.before_cursor();
        let at = before.rfind('@')?;
        let query = &before[at + 1..];
        let boundary = before[..at]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        if !boundary || query.contains(char::is_whitespace) {
            return None;
        }
        Some((before[..at].chars().count(), query.to_lowercase()))
    }

    /// Members matching the `@word` under the cursor, best first; empty
    /// when no mention is being typed.
    pub fn mention_matches(&self) -> Vec<Member> {
        if self.mention_hidden || self.focus != Focus::Composer {
            return Vec::new();
        }
        let Some((_, query)) = self.mention_query() else {
            return Vec::new();
        };
        let mut matches: Vec<(bool, &Member)> = self
            .state
            .members
            .iter()
            .filter(|member| member.user_id != self.account().user_id)
            .filter_map(|member| {
                let name = member.name.to_lowercase();
                let local = member.user_id.trim_start_matches('@').to_lowercase();
                let starts = name.starts_with(&query) || local.starts_with(&query);
                (starts || name.contains(&query)).then_some((!starts, member))
            })
            .collect();
        matches.sort_by_key(|(not_prefix, member)| (*not_prefix, member.name.to_lowercase()));
        matches
            .into_iter()
            .take(MENTION_ROWS)
            .map(|(_, member)| member.clone())
            .collect()
    }

    pub(super) fn mention_edited(&mut self) {
        self.mention_hidden = false;
        self.mention_selected = 0;
    }

    // -- emoji completion ---------------------------------------------------

    /// The `:name` being typed right before the cursor: (char index of the
    /// colon, the name so far). Only at the start or after whitespace, so
    /// times like 10:30 do not trigger it.
    fn emoji_query(&self) -> Option<(usize, String)> {
        let before = self.composer.before_cursor();
        let colon = before.rfind(':')?;
        let query = &before[colon + 1..];
        let boundary = before[..colon]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        let valid = query
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-'));
        (boundary && valid).then(|| (before[..colon].chars().count(), query.to_owned()))
    }

    /// Emoji fitting the `:name` under the cursor: (emoji, short name).
    pub fn emoji_matches(&self) -> Vec<(String, String)> {
        if self.mention_hidden || self.focus != Focus::Composer {
            return Vec::new();
        }
        match self.emoji_query() {
            Some((_, query)) => crate::emoji::find(
                &query,
                MENTION_ROWS,
                &self.emoji_usage.favourites(MENTION_ROWS),
            ),
            None => Vec::new(),
        }
    }

    // -- command completion -------------------------------------------------

    /// Commands that continue the `/name` being typed: (name, what it
    /// does, takes an argument). Only while the name itself is typed.
    pub fn command_matches(&self) -> Vec<(&'static str, &'static str, bool)> {
        let text = self.composer.text();
        let typing_name = self.focus == Focus::Composer
            && !self.mention_hidden
            && self.composer.cursor() == text.chars().count()
            && !text.contains(char::is_whitespace);
        let Some(typed) = text.strip_prefix('/').filter(|_| typing_name) else {
            return Vec::new();
        };
        let typed = typed.to_lowercase();
        COMMANDS
            .iter()
            .copied()
            .filter(|(name, _, _)| name.starts_with(&typed))
            .collect()
    }

    // -- file path completion ------------------------------------------------

    /// The path typed after `/file `, while the cursor is at its end.
    fn path_query(&self) -> Option<&str> {
        if self.mention_hidden || self.focus != Focus::Composer {
            return None;
        }
        let text = self.composer.text();
        let argument = text
            .strip_prefix("/file ")
            .or_else(|| text.strip_prefix("/upload "))?;
        (self.composer.cursor() == text.chars().count()).then_some(argument)
    }

    /// Files and folders that continue the path being typed, folders first:
    /// (name shown, text the path becomes when chosen).
    pub fn path_matches(&self) -> Vec<(String, String)> {
        let Some(argument) = self.path_query() else {
            return Vec::new();
        };
        // The popup is asked for with every frame and key: the folder is
        // read once for what is typed, and again after a moment (a slow
        // network folder would else stall every frame).
        if let Some((typed, read, matches)) = &*self.path_cache.borrow()
            && typed == argument
            && read.elapsed() < std::time::Duration::from_secs(2)
        {
            return matches.clone();
        }
        let matches = Self::paths_for(argument);
        *self.path_cache.borrow_mut() = Some((
            argument.to_owned(),
            std::time::Instant::now(),
            matches.clone(),
        ));
        matches
    }

    fn paths_for(argument: &str) -> Vec<(String, String)> {
        let (folder, prefix) = match argument.rfind('/') {
            Some(slash) => (&argument[..=slash], &argument[slash + 1..]),
            None => ("", argument),
        };
        let on_disk = match folder.strip_prefix("~/") {
            Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
            None if folder.is_empty() => std::path::PathBuf::from("."),
            None => std::path::PathBuf::from(folder),
        };
        let Ok(entries) = std::fs::read_dir(on_disk) else {
            return Vec::new();
        };

        let wanted = prefix.to_lowercase();
        let mut found: Vec<(bool, String)> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let hidden = name.starts_with('.') && !prefix.starts_with('.');
                let is_folder =
                    entry.file_type().is_ok_and(|kind| kind.is_dir()) || entry.path().is_dir();
                (!hidden && name.to_lowercase().starts_with(&wanted)).then_some((is_folder, name))
            })
            .collect();
        // A file called just what is typed comes first: Enter then sends
        // it, though other names go on from there (report.pdf.sig).
        found.sort_by_key(|(is_folder, name)| {
            (
                *is_folder || name != prefix,
                !is_folder,
                name.to_lowercase(),
            )
        });

        let matches: Vec<(String, String)> = found
            .into_iter()
            .take(MENTION_ROWS)
            .map(|(is_folder, name)| {
                let slash = if is_folder { "/" } else { "" };
                (format!("{name}{slash}"), format!("{folder}{name}{slash}"))
            })
            .collect();
        // A complete file name needs no suggestion; Enter should send it.
        if matches.len() == 1 && matches[0].1 == argument {
            return Vec::new();
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn tab_takes_the_row_the_popup_highlights() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.focus = Focus::Composer;
        for c in "/s".chars() {
            app.on_event(key(KeyCode::Char(c)));
        }
        let popup = app.popup();
        assert_eq!(popup.title, " commands · Tab ");
        assert!(popup.rows.len() > 1, "{popup:?}");
        app.on_event(key(KeyCode::Down));
        let chosen = app.popup();
        assert_eq!(chosen.selected, Some(1));
        let wanted = chosen.rows[1].0.clone();
        app.on_event(key(KeyCode::Tab));
        assert_eq!(app.composer.text().trim_end(), wanted);
    }
}
