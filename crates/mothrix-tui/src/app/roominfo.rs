// SPDX-License-Identifier: MPL-2.0

//! The room info dialog: what the room is, who is in it, and what can be
//! done there (invite, remove, ban, rename, leave).

use crossterm::event::{KeyCode, KeyEvent};

use super::{App, RoomMenu};
use crate::{
    input::Input,
    matrix::Command,
    model::{RoomDetails, RoomMember},
};

/// What is being typed in the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfoInput {
    Invite,
    Name,
    Topic,
}

/// What the dialog asks about before doing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfoConfirm {
    Kick,
    Ban,
    Leave,
}

#[derive(Clone, Debug)]
pub enum InfoMode {
    List,
    Input(InfoInput, Input),
    Confirm(InfoConfirm),
}

#[derive(Clone, Debug)]
pub struct RoomInfo {
    pub room_id: String,
    /// `None` until the backend answered.
    pub details: Option<RoomDetails>,
    /// The member the cursor is on.
    pub cursor: usize,
    pub mode: InfoMode,
}

/// What the dialog's buttons and rows do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InfoAction {
    Select(usize),
    Invite,
    Kick,
    Ban,
    Message,
    /// Compare emoji with the selected person.
    Verify,
    Rename,
    Topic,
    Leave,
    Confirm,
    Back,
}

impl RoomInfo {
    pub fn selected(&self) -> Option<&RoomMember> {
        self.details.as_ref()?.members.get(self.cursor)
    }
}

impl App {
    /// Open the dialog for a room and ask the backend about it.
    pub(crate) fn open_room_info(&mut self, room_id: String, commands: &mut Vec<Command>) {
        self.overlays.close::<RoomMenu>();
        self.overlays.open(RoomInfo {
            room_id: room_id.clone(),
            details: None,
            cursor: 0,
            mode: InfoMode::List,
        });
        commands.push(Command::RoomDetails { room_id });
    }

    pub(super) fn on_room_details(&mut self, details: RoomDetails) {
        if let Some(info) = self.overlays.get_mut::<RoomInfo>()
            && info.room_id == details.room_id
        {
            info.cursor = info.cursor.min(details.members.len().saturating_sub(1));
            info.details = Some(details);
        }
    }

    pub(super) fn room_info_action(&mut self, action: InfoAction, commands: &mut Vec<Command>) {
        let own = self.account().user_id.clone();
        let Some(info) = self.overlays.get_mut::<RoomInfo>() else {
            return;
        };
        let Some(details) = info.details.clone() else {
            if action == InfoAction::Back {
                self.overlays.close::<RoomInfo>();
            }
            return;
        };
        let selected = info.selected().cloned();
        let not_allowed = |what: &str| Some(format!("You may not {what} in this room."));
        // Removing or banning needs someone else under the cursor.
        let other = selected.as_ref().filter(|member| member.user_id != own);
        match action {
            InfoAction::Select(index) if index < details.members.len() => {
                info.cursor = index;
                info.mode = InfoMode::List;
            }
            InfoAction::Select(_) => {}
            InfoAction::Invite if !details.can_invite => self.notice = not_allowed("invite"),
            InfoAction::Invite => info.mode = InfoMode::Input(InfoInput::Invite, Input::default()),
            InfoAction::Rename if !details.can_rename => self.notice = not_allowed("rename"),
            InfoAction::Rename => {
                let mut input = Input::default();
                input.set(&details.name);
                info.mode = InfoMode::Input(InfoInput::Name, input);
            }
            InfoAction::Topic if !details.can_set_topic => {
                self.notice = not_allowed("change the topic");
            }
            InfoAction::Topic => {
                let mut input = Input::default();
                input.set(details.topic.lines().next().unwrap_or_default());
                info.mode = InfoMode::Input(InfoInput::Topic, input);
            }
            InfoAction::Kick if !details.can_kick => self.notice = not_allowed("remove people"),
            InfoAction::Ban if !details.can_ban => self.notice = not_allowed("ban people"),
            InfoAction::Kick | InfoAction::Ban if other.is_none() => {
                self.notice = Some("Choose someone else in the list first.".to_owned());
            }
            InfoAction::Kick => info.mode = InfoMode::Confirm(InfoConfirm::Kick),
            InfoAction::Ban => info.mode = InfoMode::Confirm(InfoConfirm::Ban),
            InfoAction::Leave => info.mode = InfoMode::Confirm(InfoConfirm::Leave),
            InfoAction::Verify => match other {
                Some(member) => {
                    commands.push(Command::VerifyUser {
                        user_id: member.user_id.clone(),
                    });
                    self.overlays.close::<RoomInfo>();
                }
                None => self.notice = Some("Choose someone else in the list first.".to_owned()),
            },
            InfoAction::Message => match other {
                Some(member) => {
                    commands.push(Command::StartDm {
                        user_id: member.user_id.clone(),
                    });
                    self.overlays.close::<RoomInfo>();
                }
                None => self.notice = Some("Choose someone else in the list first.".to_owned()),
            },
            InfoAction::Confirm => {
                let room_id = info.room_id.clone();
                match &info.mode {
                    InfoMode::List => {}
                    InfoMode::Input(what, input) => {
                        let typed = input.text().trim().to_owned();
                        let command = match what {
                            _ if typed.is_empty() && *what != InfoInput::Topic => None,
                            InfoInput::Invite => Some(Command::Invite {
                                room_id,
                                user_id: typed,
                            }),
                            InfoInput::Name => Some(Command::SetRoomName {
                                room_id,
                                name: typed,
                            }),
                            InfoInput::Topic => Some(Command::SetTopic {
                                room_id,
                                topic: typed,
                            }),
                        };
                        commands.extend(command);
                        info.mode = InfoMode::List;
                    }
                    InfoMode::Confirm(what) => {
                        let user_id = other.map(|member| member.user_id.clone());
                        match (what, user_id) {
                            (InfoConfirm::Leave, _) => {
                                commands.push(Command::Leave {
                                    room_id: room_id.clone(),
                                });
                                self.overlays.close::<RoomInfo>();
                                self.forget_room(&room_id);
                                return;
                            }
                            (InfoConfirm::Kick, Some(user_id)) => {
                                commands.push(Command::Kick { room_id, user_id });
                            }
                            (InfoConfirm::Ban, Some(user_id)) => {
                                commands.push(Command::Ban { room_id, user_id });
                            }
                            _ => {}
                        }
                        info.mode = InfoMode::List;
                    }
                }
            }
            InfoAction::Back => match info.mode {
                InfoMode::List => self.overlays.close::<RoomInfo>(),
                _ => info.mode = InfoMode::List,
            },
        }
    }

    /// A room we left: nothing of it stays on screen.
    pub(crate) fn forget_room(&mut self, room_id: &str) {
        if self.state.open_room.as_deref() == Some(room_id) {
            self.state.open_room = None;
            self.state.open_thread = None;
            self.state.items.clear();
            self.state.members.clear();
            self.state.typing.clear();
            self.clear_selection();
            self.focus = super::Focus::Sidebar;
        }
        if self.cursor_room.as_deref() == Some(room_id) {
            self.cursor_room = None;
        }
        self.drafts.remove(room_id);
        self.state_dirty = true;
    }

    pub(super) fn on_room_info_key(&mut self, key: KeyEvent, commands: &mut Vec<Command>) {
        let Some(info) = self.overlays.get_mut::<RoomInfo>() else {
            return;
        };
        let count = info
            .details
            .as_ref()
            .map_or(0, |details| details.members.len());
        match &mut info.mode {
            InfoMode::List => {
                let action = match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => InfoAction::Back,
                    KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                        info.cursor = info.cursor.checked_sub(1).unwrap_or(count - 1);
                        return;
                    }
                    KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                        info.cursor = (info.cursor + 1) % count;
                        return;
                    }
                    KeyCode::PageDown if count > 0 => {
                        info.cursor = (info.cursor + 10).min(count - 1);
                        return;
                    }
                    KeyCode::PageUp => {
                        info.cursor = info.cursor.saturating_sub(10);
                        return;
                    }
                    KeyCode::Char('i') => InfoAction::Invite,
                    KeyCode::Char('x') => InfoAction::Kick,
                    KeyCode::Char('b') => InfoAction::Ban,
                    KeyCode::Char('m') | KeyCode::Enter => InfoAction::Message,
                    KeyCode::Char('v') => InfoAction::Verify,
                    KeyCode::Char('n') => InfoAction::Rename,
                    KeyCode::Char('t') => InfoAction::Topic,
                    KeyCode::Char('L') => InfoAction::Leave,
                    _ => return,
                };
                self.room_info_action(action, commands);
            }
            InfoMode::Input(_, input) => match key.code {
                KeyCode::Esc => self.room_info_action(InfoAction::Back, commands),
                KeyCode::Enter => self.room_info_action(InfoAction::Confirm, commands),
                _ => {
                    input.edit(key);
                }
            },
            InfoMode::Confirm(_) => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.room_info_action(InfoAction::Confirm, commands);
                }
                _ => self.room_info_action(InfoAction::Back, commands),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{Event, KeyModifiers};

    use super::*;
    use crate::{matrix::Update, model::RoomEntry, theme};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_event(key(KeyCode::Char(c)));
        }
    }

    fn member(id: &str, name: &str, role: &str) -> RoomMember {
        RoomMember {
            user_id: id.into(),
            name: name.into(),
            role: role.into(),
            invited: false,
        }
    }

    fn details(moderator: bool) -> RoomDetails {
        RoomDetails {
            room_id: "!a".into(),
            name: "Lunch Club".into(),
            members: vec![
                member("@me:x", "Me", if moderator { "admin" } else { "" }),
                member("@amy:x", "Amy", ""),
            ],
            can_invite: true,
            can_kick: moderator,
            can_ban: moderator,
            can_rename: moderator,
            can_set_topic: moderator,
            ..RoomDetails::default()
        }
    }

    fn open(moderator: bool) -> App {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.on_update(Update::Rooms(vec![RoomEntry {
            id: "!a".into(),
            name: "Lunch Club".into(),
            ..RoomEntry::default()
        }]));
        let mut commands = Vec::new();
        app.open_room_info("!a".into(), &mut commands);
        assert!(matches!(&commands[..], [Command::RoomDetails { room_id }] if room_id == "!a"));
        app.on_update(Update::RoomDetails(details(moderator)));
        app
    }

    #[test]
    fn inviting_takes_a_typed_id() {
        let mut app = open(false);
        app.on_event(key(KeyCode::Char('i')));
        type_text(&mut app, "@ben:x");
        let commands = app.on_event(key(KeyCode::Enter));
        assert!(
            matches!(&commands[..], [Command::Invite { room_id, user_id }]
                if room_id == "!a" && user_id == "@ben:x"),
            "{commands:?}"
        );
    }

    #[test]
    fn removing_someone_needs_the_right_and_a_confirmation() {
        let mut app = open(false);
        app.on_event(key(KeyCode::Down));
        assert!(app.on_event(key(KeyCode::Char('x'))).is_empty());
        assert!(
            app.notice
                .as_deref()
                .unwrap_or_default()
                .contains("may not")
        );

        let mut app = open(true);
        // Not ourselves.
        app.on_event(key(KeyCode::Char('x')));
        assert!(matches!(
            app.overlays.get::<RoomInfo>().unwrap().mode,
            InfoMode::List
        ));
        app.on_event(key(KeyCode::Down));
        assert!(app.on_event(key(KeyCode::Char('x'))).is_empty());
        let commands = app.on_event(key(KeyCode::Char('y')));
        assert!(
            matches!(&commands[..], [Command::Kick { user_id, .. }] if user_id == "@amy:x"),
            "{commands:?}"
        );
    }

    #[test]
    fn leaving_closes_the_room() {
        let mut app = open(false);
        app.state.open_room = Some("!a".into());
        app.on_event(key(KeyCode::Char('L')));
        let commands = app.on_event(key(KeyCode::Char('y')));
        assert!(matches!(&commands[..], [Command::Leave { room_id }] if room_id == "!a"));
        assert!(!app.overlays.is_open::<RoomInfo>());
        assert_eq!(app.state.open_room, None);
    }

    #[test]
    fn m_starts_a_direct_chat_with_the_selected_member() {
        let mut app = open(false);
        app.on_event(key(KeyCode::Down));
        let commands = app.on_event(key(KeyCode::Char('m')));
        assert!(matches!(&commands[..], [Command::StartDm { user_id }] if user_id == "@amy:x"));
    }
}
