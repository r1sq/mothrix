// SPDX-License-Identifier: MPL-2.0

//! What [`App`] asks of the world outside it: the backend, the terminal,
//! the desktop and the files. All of it leaves through one list,
//! [`App::take_effects`], in the order it came up; the main loop and
//! snapshot mode are the two places that carry effects out.

use super::*;

/// One thing for the world outside [`App`] to do.
#[derive(Debug)]
pub enum Effect {
    /// For the backend of the account it is about. Commands that a key
    /// press or click caused come back from [`App::on_event`] instead.
    Command(Command),
    /// Escape sequences (picture uploads and removals) to write to the
    /// terminal before the next frame.
    Terminal(String),
    /// Tell the desktop about new messages in a room (its key).
    Notify {
        room: String,
        title: String,
    },
    /// A room (its key) was opened: its notifications are read.
    RoomOpened(String),
    /// Put text on the clipboard.
    Copy(String),
    /// Check the text in the message box for grammar, after a pause.
    CheckGrammar(String),
    /// Keep a word in the list of words that are not misspelled.
    IgnoreWord(String),
    /// Count an emoji the user took in the file of favourites.
    UsedEmoji(String),
    /// Sign in or out (the accounts dialog).
    Account(AccountRequest),
    /// Look for a new release of mothrix (`/update`, nothing known yet).
    CheckForUpdate,
    /// Download this release, check it and put it in place.
    InstallUpdate(crate::update::Available),
    /// The drafts or the open room changed.
    SaveState,
    /// A setting changed.
    SaveConfig,
    Quit,
}

impl App {
    /// Everything to be done outside since the last call, in order.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        let mut effects = std::mem::take(&mut self.effects);
        if let Some(text) = self.grammar_request() {
            effects.push(Effect::CheckGrammar(text));
        }
        if std::mem::take(&mut self.state_dirty) {
            effects.push(Effect::SaveState);
        }
        if std::mem::take(&mut self.state.config_dirty) {
            effects.push(Effect::SaveConfig);
        }
        if self.should_quit {
            effects.push(Effect::Quit);
        }
        effects
    }

    /// A command that came up outside a key press or click.
    pub(super) fn send_later(&mut self, command: Command) {
        self.effects.push(Effect::Command(command));
    }

    /// Open a room because its notification was clicked.
    pub fn open_from_notification(&mut self, room: String) {
        if !self.state.rooms.iter().any(|known| known.id == room) {
            return;
        }
        let mut commands = Vec::new();
        self.open(room, &mut commands);
        self.effects
            .extend(commands.into_iter().map(Effect::Command));
    }
}

#[cfg(test)]
impl App {
    /// Take the effects `pick` wants, leaving the others queued.
    fn take_some<T>(&mut self, mut pick: impl FnMut(&Effect) -> Option<T>) -> Vec<T> {
        let mut picked = Vec::new();
        self.effects.retain(|effect| match pick(effect) {
            Some(value) => {
                picked.push(value);
                false
            }
            None => true,
        });
        picked
    }

    /// The queued commands, in order (they are not `Clone`, so this does
    /// not go through `take_some`).
    pub fn take_commands(&mut self) -> Vec<Command> {
        let mut taken = Vec::new();
        let mut kept = Vec::new();
        for effect in std::mem::take(&mut self.effects) {
            match effect {
                Effect::Command(command) => taken.push(command),
                other => kept.push(other),
            }
        }
        self.effects = kept;
        taken
    }

    pub fn take_grammar_request(&mut self) -> Option<String> {
        self.grammar_request()
    }

    pub fn take_graphics(&mut self) -> Vec<String> {
        self.take_some(|effect| match effect {
            Effect::Terminal(sequence) => Some(sequence.clone()),
            _ => None,
        })
    }

    pub fn take_clipboard(&mut self) -> Option<String> {
        self.take_some(|effect| match effect {
            Effect::Copy(text) => Some(text.clone()),
            _ => None,
        })
        .pop()
    }

    pub fn take_new_ignored_words(&mut self) -> Vec<String> {
        self.take_some(|effect| match effect {
            Effect::IgnoreWord(word) => Some(word.clone()),
            _ => None,
        })
    }

    pub fn take_account_request(&mut self) -> Option<AccountRequest> {
        self.take_some(|effect| match effect {
            Effect::Account(request) => Some(request.clone()),
            _ => None,
        })
        .pop()
    }

    pub fn take_notifications(&mut self) -> Vec<String> {
        self.take_some(|effect| match effect {
            Effect::Notify { title, .. } => Some(title.clone()),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(id: &str, name: &str, unread: u64) -> RoomEntry {
        RoomEntry {
            id: id.into(),
            name: name.into(),
            unread,
            ..RoomEntry::default()
        }
    }

    #[test]
    fn effects_leave_in_order_and_flags_once() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.send_later(Command::Typing(true));
        app.effects.push(Effect::Copy("x".into()));
        app.state.config_dirty = true;
        let effects = app.take_effects();
        assert!(matches!(
            effects.as_slice(),
            [
                Effect::Command(Command::Typing(true)),
                Effect::Copy(_),
                Effect::SaveConfig
            ]
        ));
        assert!(app.take_effects().is_empty());
    }

    #[test]
    fn a_clicked_notification_opens_its_room() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.on_update(Update::Rooms(vec![
            room("!a", "Lunch", 0),
            room("!b", "Dev", 2),
        ]));
        app.open_from_notification("!b".into());
        assert_eq!(app.state.open_room.as_deref(), Some("!b"));
        assert_eq!(app.focus, Focus::Composer);
        assert!(
            app.take_effects()
                .iter()
                .any(|effect| matches!(effect, Effect::Command(Command::Open(id)) if id == "!b"))
        );
        // A room that is gone by now is left alone.
        app.open_from_notification("!gone".into());
        assert_eq!(app.state.open_room.as_deref(), Some("!b"));
    }

    #[test]
    fn opening_a_room_reads_its_notifications_once() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.on_update(Update::Rooms(vec![room("!a", "Lunch", 1)]));
        let opened = |app: &mut App| -> Vec<String> {
            app.take_effects()
                .into_iter()
                .filter_map(|effect| match effect {
                    Effect::RoomOpened(room) => Some(room),
                    _ => None,
                })
                .collect()
        };
        app.on_hit(HitAction::OpenRoom("!a".into()), &mut Vec::new());
        assert_eq!(opened(&mut app), ["!a"]);
        // Again: it is open already.
        app.on_hit(HitAction::OpenRoom("!a".into()), &mut Vec::new());
        assert!(opened(&mut app).is_empty());
    }
}
