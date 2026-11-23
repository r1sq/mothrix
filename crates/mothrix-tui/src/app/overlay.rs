// SPDX-License-Identifier: MPL-2.0

//! The dialogs and menus that sit over the main screen.
//!
//! They are kept as a stack: the last one is on top, is drawn last and gets
//! every key, pasted text and click. Most of the time there is one at most;
//! a verification request or a new recovery key can arrive while another is
//! open and then sits on top of it until it is done.
//!
//! A new kind of dialog is a new [`Overlay`] variant. The compiler then asks
//! for its keys ([`App::overlay_key`]), pasted text ([`Overlay::paste`]) and
//! drawing (`ui::draw_overlays`); whether a click outside closes it is
//! [`Overlay::closes_on_outside_click`].

use crossterm::event::KeyCode;

use super::*;

/// One dialog or menu over the main screen, with its state.
#[derive(Clone, Debug)]
pub enum Overlay {
    Settings(SettingsDialog),
    Accounts(AccountsDialog),
    RoomInfo(RoomInfo),
    Forward(Forward),
    RecoveryKey(RecoveryKey),
    ResetPrompt(ResetPrompt),
    RoomMenu(RoomMenu),
    SectionManager(SectionManager),
    SectionPicker(SectionPicker),
    Search(Search),
    Verify(VerifyState),
    EmojiPicker(EmojiPicker),
    Help(Help),
    FormatMenu(FormatMenu),
}

/// The settings dialog.
#[derive(Clone, Debug, Default)]
pub struct SettingsDialog {
    /// The row under the cursor (an index into [`Setting::ALL`]).
    pub cursor: usize,
    /// The text being typed for the row under the cursor.
    pub edit: Option<Input>,
}

/// A new recovery key, shown until closed.
#[derive(Clone, Debug)]
pub struct RecoveryKey(pub String);

/// The password being typed to confirm an identity reset.
#[derive(Clone, Debug, Default)]
pub struct ResetPrompt(pub Input);

impl Overlay {
    /// Whether a click outside closes it. The recovery key is shown only
    /// once, and a verification has someone waiting on the other end: those
    /// close only on purpose.
    pub fn closes_on_outside_click(&self) -> bool {
        !matches!(self, Self::RecoveryKey(_) | Self::Verify(_))
    }

    /// Put pasted text into the field being typed in, if there is one.
    /// Pasted text never goes past an open dialog to the message box.
    pub fn paste(&mut self, text: &str) {
        let text = text.trim();
        match self {
            Self::Settings(settings) => {
                if let Some(input) = settings.edit.as_mut() {
                    input.insert_str(text);
                }
            }
            Self::Accounts(dialog) => match &mut dialog.mode {
                AccountsMode::Rename(input) => input.insert_str(text),
                AccountsMode::Add(form) if !form.busy => {
                    form.fields[form.field].insert_str(text);
                }
                _ => {}
            },
            Self::RoomInfo(info) => {
                if let InfoMode::Input(_, input) = &mut info.mode {
                    input.insert_str(text);
                }
            }
            Self::Forward(forward) => {
                forward.filter.insert_str(text);
                forward.cursor = 0;
            }
            Self::ResetPrompt(prompt) => prompt.0.insert_str(text),
            Self::SectionManager(manager) => {
                if let Some(input) = manager.renaming.as_mut() {
                    input.insert_str(text);
                }
            }
            Self::SectionPicker(picker) => {
                if let Some(input) = picker.naming.as_mut() {
                    input.insert_str(text);
                }
            }
            Self::Search(search) => search.input.insert_str(text),
            Self::EmojiPicker(picker) => {
                picker.search.insert_str(text);
                picker.searched();
            }
            Self::Help(help) => {
                help.filter.insert_str(text);
                help.scroll = 0;
            }
            Self::RecoveryKey(_) | Self::RoomMenu(_) | Self::Verify(_) | Self::FormatMenu(_) => {}
        }
    }
}

/// The open dialogs, the one on top last.
#[derive(Clone, Debug, Default)]
pub struct Overlays(Vec<Overlay>);

/// A type of state that is one kind of [`Overlay`].
pub trait OverlayKind: Sized {
    fn of(overlay: &Overlay) -> Option<&Self>;
    fn of_mut(overlay: &mut Overlay) -> Option<&mut Self>;
    fn from(overlay: Overlay) -> Option<Self>;
}

macro_rules! overlay_kinds {
    ($($variant:ident($kind:ty)),* $(,)?) => {$(
        impl OverlayKind for $kind {
            fn of(overlay: &Overlay) -> Option<&Self> {
                match overlay {
                    Overlay::$variant(state) => Some(state),
                    _ => None,
                }
            }
            fn of_mut(overlay: &mut Overlay) -> Option<&mut Self> {
                match overlay {
                    Overlay::$variant(state) => Some(state),
                    _ => None,
                }
            }
            fn from(overlay: Overlay) -> Option<Self> {
                match overlay {
                    Overlay::$variant(state) => Some(state),
                    _ => None,
                }
            }
        }
        impl From<$kind> for Overlay {
            fn from(state: $kind) -> Self {
                Overlay::$variant(state)
            }
        }
    )*};
}

overlay_kinds!(
    Settings(SettingsDialog),
    Accounts(AccountsDialog),
    RoomInfo(RoomInfo),
    Forward(Forward),
    RecoveryKey(RecoveryKey),
    ResetPrompt(ResetPrompt),
    RoomMenu(RoomMenu),
    SectionManager(SectionManager),
    SectionPicker(SectionPicker),
    Search(Search),
    Verify(VerifyState),
    EmojiPicker(EmojiPicker),
    Help(Help),
    FormatMenu(FormatMenu),
);

impl Overlays {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The open dialogs, the one on top last.
    pub fn iter(&self) -> impl Iterator<Item = &Overlay> {
        self.0.iter()
    }

    pub fn top(&self) -> Option<&Overlay> {
        self.0.last()
    }

    /// Show a dialog on top. One of its kind that is open already is
    /// replaced where it is (a verification moving on to its next step).
    pub fn open(&mut self, overlay: impl Into<Overlay>) {
        let overlay = overlay.into();
        let kind = std::mem::discriminant(&overlay);
        match self
            .0
            .iter_mut()
            .find(|open| std::mem::discriminant(*open) == kind)
        {
            Some(open) => *open = overlay,
            None => self.0.push(overlay),
        }
    }

    pub fn is_open<T: OverlayKind>(&self) -> bool {
        self.get::<T>().is_some()
    }

    pub fn get<T: OverlayKind>(&self) -> Option<&T> {
        self.0.iter().find_map(T::of)
    }

    pub fn get_mut<T: OverlayKind>(&mut self) -> Option<&mut T> {
        self.0.iter_mut().find_map(T::of_mut)
    }

    /// Close the dialog of this kind and hand back its state.
    pub fn take<T: OverlayKind>(&mut self) -> Option<T> {
        let at = self.0.iter().position(|open| T::of(open).is_some())?;
        T::from(self.0.remove(at))
    }

    /// Close the dialog of this kind, if it is open.
    pub fn close<T: OverlayKind>(&mut self) {
        self.take::<T>();
    }

    /// A click outside the dialog on top: close it, if it closes that way.
    pub fn click_outside(&mut self) {
        if self.0.last().is_some_and(Overlay::closes_on_outside_click) {
            self.0.pop();
        }
    }
}

type KeyHandler = fn(&mut App, KeyEvent, &mut Vec<Command>);

impl App {
    /// Hand a key to the dialog on top. False when none is open.
    pub(super) fn overlay_key(&mut self, key: KeyEvent, commands: &mut Vec<Command>) -> bool {
        let Some(top) = self.overlays.top() else {
            return false;
        };
        let handler: KeyHandler = match top {
            Overlay::Settings(_) => |app, key, _| app.on_settings_key(key),
            Overlay::Accounts(_) => |app, key, _| app.on_accounts_key(key),
            Overlay::RoomInfo(_) => App::on_room_info_key,
            Overlay::Forward(_) => App::on_forward_key,
            Overlay::RecoveryKey(_) => |app, key, _| app.on_recovery_key(key),
            Overlay::ResetPrompt(_) => App::on_reset_key,
            Overlay::RoomMenu(_) => App::on_room_menu_key,
            Overlay::SectionManager(_) => |app, key, _| app.on_manager_key(key),
            Overlay::SectionPicker(_) => |app, key, _| app.on_section_key(key),
            Overlay::Search(_) => App::on_search_key,
            Overlay::Verify(_) => App::on_verify_key,
            Overlay::EmojiPicker(_) => App::on_emoji_picker_key,
            Overlay::Help(_) => |app, key, _| app.on_help_key(key),
            Overlay::FormatMenu(_) => |app, key, _| app.on_format_menu_key(key),
        };
        handler(self, key, commands);
        true
    }

    /// Hand pasted text to the dialog on top. False when none is open.
    pub(super) fn overlay_paste(&mut self, text: &str) -> bool {
        match self.overlays.0.last_mut() {
            Some(top) => {
                top.paste(text);
                true
            }
            None => false,
        }
    }

    fn on_recovery_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('c') => {
                if let Some(RecoveryKey(recovery_key)) = self.overlays.get() {
                    self.effects.push(Effect::Copy(recovery_key.clone()));
                    self.notice = Some("Recovery key copied to the clipboard.".to_owned());
                }
            }
            KeyCode::Esc | KeyCode::Enter => self.overlays.close::<RecoveryKey>(),
            _ => {}
        }
    }

    fn on_reset_key(&mut self, key: KeyEvent, commands: &mut Vec<Command>) {
        let Some(ResetPrompt(input)) = self.overlays.get_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.overlays.close::<ResetPrompt>(),
            KeyCode::Enter => {
                let password = input.text().to_owned();
                self.overlays.close::<ResetPrompt>();
                self.notice = Some("Resetting your identity…".to_owned());
                commands.push(Command::ResetIdentity { password });
            }
            _ => {
                input.edit(key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    use super::*;
    use crate::{model::RoomEntry, theme};

    fn app() -> App {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.on_update(Update::Rooms(vec![RoomEntry {
            id: "!a".into(),
            name: "Lunch Club".into(),
            ..RoomEntry::default()
        }]));
        app.on_event(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.state.open_room.as_deref(), Some("!a"));
        app
    }

    /// Every kind of dialog, each with a field being typed in where it has one.
    fn one_of_each() -> Vec<Overlay> {
        let all: Vec<Overlay> = vec![
            SettingsDialog {
                cursor: Setting::LanguageToolServer.index(),
                edit: Some(Input::default()),
            }
            .into(),
            AccountsDialog {
                cursor: 0,
                mode: AccountsMode::Rename(Input::default()),
            }
            .into(),
            RoomInfo {
                room_id: "!a".into(),
                details: None,
                cursor: 0,
                mode: InfoMode::List,
            }
            .into(),
            Forward {
                event_id: "$1".into(),
                filter: Input::default(),
                cursor: 0,
            }
            .into(),
            RecoveryKey("EsTc abcd".into()).into(),
            ResetPrompt::default().into(),
            RoomMenu {
                room_id: "!a".into(),
                x: 5,
                y: 3,
                cursor: 0,
            }
            .into(),
            SectionManager::default().into(),
            SectionPicker {
                room_id: "!a".into(),
                cursor: 0,
                naming: Some(Input::default()),
            }
            .into(),
            Search::default().into(),
            VerifyState::Incoming {
                device: "phone".into(),
            }
            .into(),
            EmojiPicker::new(PickFor::Insert).into(),
            Help::default().into(),
            FormatMenu::default().into(),
        ];
        // A new kind of dialog stops this from compiling until it is in
        // the list above.
        for overlay in &all {
            match overlay {
                Overlay::Settings(_)
                | Overlay::Accounts(_)
                | Overlay::RoomInfo(_)
                | Overlay::Forward(_)
                | Overlay::RecoveryKey(_)
                | Overlay::ResetPrompt(_)
                | Overlay::RoomMenu(_)
                | Overlay::SectionManager(_)
                | Overlay::SectionPicker(_)
                | Overlay::Search(_)
                | Overlay::Verify(_)
                | Overlay::EmojiPicker(_)
                | Overlay::Help(_)
                | Overlay::FormatMenu(_) => {}
            }
        }
        all
    }

    #[test]
    fn pasted_text_never_reaches_the_message_box_under_a_dialog() {
        for overlay in one_of_each() {
            let mut app = app();
            app.overlays.open(overlay.clone());
            app.on_event(Event::Paste("hello".into()));
            assert!(app.composer.is_empty(), "{overlay:?}");
            assert!(!app.overlays.is_empty(), "{overlay:?}");
        }
    }

    #[test]
    fn a_password_pasted_into_the_reset_prompt_stays_there() {
        let mut app = app();
        app.overlays.open(ResetPrompt::default());
        app.on_event(Event::Paste("hunter2\n".into()));
        let commands = app.on_event(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        assert!(matches!(
            commands.as_slice(),
            [Command::ResetIdentity { password }] if password == "hunter2"
        ));
        assert!(app.composer.is_empty());
    }

    #[test]
    fn typed_keys_never_reach_the_message_box_under_a_dialog() {
        for overlay in one_of_each() {
            let mut app = app();
            app.overlays.open(overlay.clone());
            app.on_event(Event::Key(KeyEvent::new(
                KeyCode::Char('z'),
                KeyModifiers::NONE,
            )));
            assert!(app.composer.is_empty(), "{overlay:?}");
        }
    }

    #[test]
    fn a_click_on_the_message_box_under_a_dialog_does_not_move_its_cursor() {
        for overlay in one_of_each() {
            let mut app = app();
            app.composer.set("hello");
            // Tall enough that no dialog covers the message box.
            crate::ui::render_to_text(&mut app, 100, 40);
            let text = app.composer_text_area;
            app.overlays.open(overlay.clone());
            crate::ui::render_to_text(&mut app, 100, 40);
            // Whether the click lands on the dialog itself (the help is as
            // tall as the screen).
            let on_dialog = app.hits.iter().any(|hit| {
                hit.action == HitAction::Blocked
                    && hit
                        .rect
                        .contains(ratatui::layout::Position::new(text.x, text.y))
            });
            app.on_event(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: text.x,
                row: text.y,
                modifiers: KeyModifiers::NONE,
            }));
            assert_eq!(app.composer.cursor(), 5, "{overlay:?}");
            // A click outside closes the dialog, unless it shows something
            // that must not go away by accident.
            assert_eq!(
                app.overlays.is_empty(),
                !on_dialog && overlay.closes_on_outside_click(),
                "{overlay:?}"
            );
        }
    }

    #[test]
    fn the_wheel_does_not_scroll_what_is_under_a_dialog() {
        let mut app = app();
        crate::ui::render_to_text(&mut app, 100, 30);
        let sidebar = app.sidebar_area;
        app.sidebar_scroll = 1;
        app.overlays.open(SettingsDialog::default());
        app.on_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: sidebar.x + 1,
            row: sidebar.y + 1,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(app.sidebar_scroll, 1);
    }

    #[test]
    fn a_room_menu_closes_when_its_room_goes_away() {
        let mut app = app();
        app.overlays.open(RoomMenu {
            room_id: "!a".into(),
            ..RoomMenu::default()
        });
        app.on_update(Update::Rooms(Vec::new()));
        assert!(app.overlays.is_empty());
    }

    #[test]
    fn a_verification_sits_on_top_of_an_open_dialog_until_it_ends() {
        let mut app = app();
        app.on_event(Event::Key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE)));
        app.on_update(Update::Verify(Some(VerifyState::Incoming {
            device: "phone".into(),
        })));
        app.on_update(Update::Verify(Some(VerifyState::Waiting)));
        let kinds: Vec<_> = app.overlays.iter().collect();
        assert!(matches!(
            kinds.as_slice(),
            [Overlay::Settings(_), Overlay::Verify(VerifyState::Waiting)]
        ));
        // A click outside does not reach past it to the settings.
        app.overlays.click_outside();
        assert!(app.overlays.is_open::<VerifyState>());

        app.on_update(Update::Verify(None));
        assert!(matches!(app.overlays.top(), Some(Overlay::Settings(_))));
        app.overlays.click_outside();
        assert!(app.overlays.is_empty());
    }
}
