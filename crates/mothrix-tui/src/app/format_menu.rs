// SPDX-License-Identifier: MPL-2.0

//! The format menu (`F6`, the `+ format` button): Markdown for the message
//! box, applied at the cursor ([`crate::format`]).

use super::*;
use crate::format::Format;

/// The open format menu: where it was asked for, and its cursor.
#[derive(Clone, Debug, Default)]
pub struct FormatMenu {
    pub x: u16,
    pub y: u16,
    pub cursor: usize,
}

impl App {
    /// Open the menu at (`x`, `y`): where the button was clicked.
    pub(super) fn open_format_menu(&mut self, x: u16, y: u16) {
        self.overlays.open(FormatMenu { x, y, cursor: 0 });
    }

    /// Open it from the keyboard: over the right end of the message box.
    pub(super) fn open_format_menu_at_composer(&mut self) {
        let area = self.composer_text_area;
        self.open_format_menu(area.right().saturating_sub(30), area.y.saturating_sub(1));
    }

    /// Apply a format to the message box, at its cursor.
    pub(crate) fn apply_format(&mut self, format: Format) {
        self.overlays.close::<FormatMenu>();
        let (text, cursor) =
            crate::format::apply(self.composer.text(), self.composer.cursor(), format);
        self.composer.set(&text);
        self.composer.set_cursor(cursor);
        self.focus = Focus::Composer;
    }

    pub(super) fn on_format_menu_key(&mut self, key: KeyEvent) {
        let rows = Format::ALL.len();
        let Some(menu) = self.overlays.get_mut::<FormatMenu>() else {
            return;
        };
        match key.code {
            KeyCode::Up => menu.cursor = menu.cursor.checked_sub(1).unwrap_or(rows - 1),
            KeyCode::Down | KeyCode::Tab => menu.cursor = (menu.cursor + 1) % rows,
            KeyCode::Enter | KeyCode::Char(' ') => {
                let format = Format::ALL[menu.cursor.min(rows - 1)];
                self.apply_format(format);
            }
            KeyCode::Char(c) => match Format::ALL.iter().find(|format| format.key() == c) {
                Some(format) => self.apply_format(*format),
                None => self.overlays.close::<FormatMenu>(),
            },
            _ => self.overlays.close::<FormatMenu>(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f6_and_a_letter_format_the_word_at_the_cursor() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.focus = Focus::Composer;
        app.composer.set("make it bold");
        app.on_event(Event::Key(KeyEvent::from(KeyCode::F(6))));
        assert!(app.overlays.is_open::<FormatMenu>());
        app.on_event(Event::Key(KeyEvent::from(KeyCode::Char('b'))));
        assert!(!app.overlays.is_open::<FormatMenu>());
        assert_eq!(app.composer.text(), "make it **bold**");
        assert_eq!(app.focus, Focus::Composer);

        // From the list with the arrows: the second entry is italic.
        app.on_event(Event::Key(KeyEvent::from(KeyCode::F(6))));
        app.on_event(Event::Key(KeyEvent::from(KeyCode::Down)));
        app.on_event(Event::Key(KeyEvent::from(KeyCode::Enter)));
        assert_eq!(app.composer.text(), "make it _**bold**_");
        // Any other key just closes it.
        app.on_event(Event::Key(KeyEvent::from(KeyCode::F(6))));
        app.on_event(Event::Key(KeyEvent::from(KeyCode::Esc)));
        assert!(!app.overlays.is_open::<FormatMenu>());
        assert_eq!(app.composer.text(), "make it _**bold**_");
    }
}

#[cfg(test)]
mod double_click_tests {
    use super::*;

    fn app_with(kind: ItemKind) -> App {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.state.open_room = Some("!a".into());
        app.state.items = vec![Item {
            key: "$f".into(),
            event_id: "$f".into(),
            body: "photo.jpg https://x.example".into(),
            kind,
            ..Item::default()
        }];
        app
    }

    fn click(app: &mut App) -> Vec<Command> {
        let mut commands = Vec::new();
        app.on_hit(HitAction::Select("$f".into()), &mut commands);
        commands
    }

    #[test]
    fn a_double_click_opens_a_file_or_picture() {
        let mut app = app_with(ItemKind::Media);
        assert!(click(&mut app).is_empty());
        assert_eq!(app.selected.as_deref(), Some("$f"));
        let commands = click(&mut app);
        assert!(
            matches!(&commands[..], [Command::OpenMedia { event_id }] if event_id == "$f"),
            "{commands:?}"
        );
        // Still selected; a third click is a single one again.
        assert_eq!(app.selected.as_deref(), Some("$f"));
        assert!(click(&mut app).is_empty());
        assert_eq!(app.selected, None);
    }

    #[test]
    fn slow_clicks_and_text_messages_open_nothing() {
        let mut app = app_with(ItemKind::Media);
        click(&mut app);
        app.last_click = app
            .last_click
            .take()
            .map(|(id, at)| (id, at - std::time::Duration::from_secs(1)));
        assert!(click(&mut app).is_empty());
        assert_eq!(app.selected, None);

        // Text with a link: a double-click only selects it.
        let mut app = app_with(ItemKind::Text);
        click(&mut app);
        assert!(click(&mut app).is_empty());
        assert_eq!(app.selected.as_deref(), Some("$f"));
    }
}
