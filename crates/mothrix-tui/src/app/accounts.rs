// SPDX-License-Identifier: MPL-2.0

//! The accounts dialog: see who is signed in, add an account, give one a
//! name of its own, sign one out. Signing in and out themselves happen in
//! the main loop, which picks up an [`AccountRequest`] and reports back.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Effect, Focus, SettingsDialog};
use crate::accounts::FormProblem;
use crate::input::Input;
use crate::matrix::Update;

/// The fields of the form for adding an account.
pub const ADD_FIELDS: [&str; 4] = ["Server", "User", "Password", "Name"];
const SERVER: usize = 0;
const USER: usize = 1;
const PASSWORD: usize = 2;
const NAME: usize = 3;

#[derive(Clone, Debug, Default)]
pub struct AddForm {
    pub fields: [Input; 4],
    pub field: usize,
    /// Signing in is under way.
    pub busy: bool,
    /// What happened last: progress, or why it did not work.
    pub status: String,
    pub failed: bool,
}

#[derive(Clone, Debug)]
pub enum AccountsMode {
    List,
    /// Typing a new name for the account under the cursor.
    Rename(Input),
    Add(Box<AddForm>),
    /// Asking whether to sign out the account under the cursor.
    Remove,
}

#[derive(Clone, Debug)]
pub struct AccountsDialog {
    pub cursor: usize,
    pub mode: AccountsMode,
}

/// What the dialog's buttons and rows do when clicked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountsAction {
    Select(usize),
    Add,
    Rename,
    Remove,
    /// Move the account under the cursor up (`true`) or down the list.
    Move(bool),
    Field(usize),
    /// Enter: sign in, save the name, or confirm signing out.
    Confirm,
    Back,
}

/// Work for the main loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountRequest {
    Add(crate::accounts::SignIn),
    /// Sign out the account at this index of [`App::accounts`].
    Remove(usize),
    /// Let these two accounts change places in the list.
    Swap(usize, usize),
}

/// The terminal UI is what the accounts are shown in.
impl crate::accounts::Frontend for App {
    fn state(&self) -> &crate::client_state::ClientState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut crate::client_state::ClientState {
        &mut self.state
    }

    fn on_update(&mut self, update: Update) {
        App::on_update(self, update);
    }

    fn account_hint(&self) -> Option<usize> {
        let cursor = self.cursor_room.as_ref()?;
        let room = self.state.rooms.iter().find(|room| room.id == *cursor)?;
        Some(room.account)
    }

    fn account_gone(&mut self, closed: bool) {
        App::account_gone(self, closed);
    }
}

impl App {
    pub fn open_accounts(&mut self) {
        self.overlays.close::<SettingsDialog>();
        self.overlays.open(AccountsDialog {
            cursor: self.active_account(),
            mode: AccountsMode::List,
        });
    }

    /// A line about the sign-in under way (like the address to open).
    pub fn account_progress(&mut self, text: String) {
        if let Some(AccountsDialog {
            mode: AccountsMode::Add(form),
            ..
        }) = self.overlays.get_mut::<AccountsDialog>()
        {
            form.status = text;
            form.failed = false;
        }
    }

    /// The sign-in the form asked for has ended. On success the account
    /// is already in [`App::accounts`], as the last one.
    pub fn account_added(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                let newest = self.state.accounts.len().saturating_sub(1);
                if let Some(dialog) = self.overlays.get_mut::<AccountsDialog>() {
                    dialog.cursor = newest;
                    dialog.mode = AccountsMode::List;
                }
                // By its name here: the id can be very long.
                self.notice = Some(format!(
                    "Account '{}' added.",
                    self.state.accounts[newest].label
                ));
            }
            Err(error) => {
                if let Some(AccountsDialog {
                    mode: AccountsMode::Add(form),
                    ..
                }) = self.overlays.get_mut::<AccountsDialog>()
                {
                    form.busy = false;
                    form.failed = true;
                    form.status = error;
                } else {
                    self.notice = Some(format!("Sign-in failed: {error}"));
                }
            }
        }
    }

    /// A signed-out account is gone from the state (`closed`: with the
    /// open room): let go of what was shown of it.
    pub fn account_gone(&mut self, closed: bool) {
        if closed {
            self.previews.clear();
            self.sixel.clear();
            for (_, id) in self.terminal_images.drain() {
                self.effects
                    .push(Effect::Terminal(crate::kitty::delete(id)));
            }
            self.clear_selection();
            self.cancel_compose();
            self.focus = Focus::Sidebar;
        }
        self.cursor_room = None;
        let last = self.state.accounts.len().saturating_sub(1);
        if let Some(dialog) = self.overlays.get_mut::<AccountsDialog>() {
            dialog.cursor = dialog.cursor.min(last);
            dialog.mode = AccountsMode::List;
        }
    }

    fn submit_new_account(&mut self) {
        let taken: Vec<String> = self
            .state
            .accounts
            .iter()
            .map(|account| account.profile.clone())
            .collect();
        let Some(AccountsDialog {
            mode: AccountsMode::Add(form),
            ..
        }) = self.overlays.get_mut::<AccountsDialog>()
        else {
            return;
        };
        if form.busy {
            return;
        }
        let request = match crate::accounts::SignIn::from_form(
            form.fields[SERVER].text(),
            form.fields[USER].text(),
            form.fields[PASSWORD].text(),
            form.fields[NAME].text(),
            &taken,
        ) {
            Ok(request) => request,
            Err(problem) => {
                form.field = match problem {
                    FormProblem::Server(_) => SERVER,
                    FormProblem::Name(_) => NAME,
                };
                form.failed = true;
                form.status = problem.text().to_owned();
                return;
            }
        };
        form.busy = true;
        form.failed = false;
        form.status = "Signing in…".to_owned();
        // Not kept longer than needed.
        form.fields[PASSWORD].clear();
        self.effects
            .push(Effect::Account(AccountRequest::Add(request)));
    }

    pub(super) fn accounts_action(&mut self, action: AccountsAction) {
        let count = self.state.accounts.len();
        let Some(dialog) = self.overlays.get_mut::<AccountsDialog>() else {
            return;
        };
        match action {
            AccountsAction::Select(index) if index < count => {
                dialog.cursor = index;
                dialog.mode = AccountsMode::List;
            }
            AccountsAction::Select(_) => {}
            AccountsAction::Add => dialog.mode = AccountsMode::Add(Box::default()),
            AccountsAction::Rename => {
                let mut input = Input::default();
                input.set(&self.state.accounts[dialog.cursor].label);
                dialog.mode = AccountsMode::Rename(input);
            }
            AccountsAction::Move(up) => {
                let from = dialog.cursor;
                let to = if up {
                    from.checked_sub(1)
                } else {
                    Some(from + 1)
                };
                if let Some(to) = to.filter(|to| *to < count) {
                    dialog.cursor = to;
                    self.effects
                        .push(Effect::Account(AccountRequest::Swap(from, to)));
                }
            }
            AccountsAction::Remove if count < 2 => {
                self.notice = Some(
                    "This is the only account; sign it out with `mothrix logout` instead."
                        .to_owned(),
                );
            }
            AccountsAction::Remove => dialog.mode = AccountsMode::Remove,
            AccountsAction::Field(field) => {
                if let AccountsMode::Add(form) = &mut dialog.mode
                    && field < ADD_FIELDS.len()
                    && !form.busy
                {
                    form.field = field;
                }
            }
            AccountsAction::Confirm => match &dialog.mode {
                AccountsMode::List => {}
                AccountsMode::Add(_) => self.submit_new_account(),
                AccountsMode::Rename(input) => {
                    let typed = input.text().trim().to_owned();
                    let account = &mut self.state.accounts[dialog.cursor];
                    // Nothing typed: back to the profile's name.
                    account.label = if typed.is_empty() {
                        account.profile.clone()
                    } else {
                        typed
                    };
                    dialog.mode = AccountsMode::List;
                    self.state.config_dirty = true;
                }
                AccountsMode::Remove => {
                    self.effects
                        .push(Effect::Account(AccountRequest::Remove(dialog.cursor)));
                    dialog.mode = AccountsMode::List;
                }
            },
            AccountsAction::Back => match &dialog.mode {
                AccountsMode::List => self.overlays.close::<AccountsDialog>(),
                // A sign-in under way goes on; the form just closes.
                _ => dialog.mode = AccountsMode::List,
            },
        }
    }

    pub(super) fn on_accounts_key(&mut self, key: KeyEvent) {
        let count = self.state.accounts.len();
        let Some(dialog) = self.overlays.get_mut::<AccountsDialog>() else {
            return;
        };
        let edit = |input: &mut Input, key: KeyEvent| {
            input.edit(key);
        };
        match &mut dialog.mode {
            AccountsMode::List => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.overlays.close::<AccountsDialog>(),
                KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.accounts_action(AccountsAction::Move(true));
                }
                KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.accounts_action(AccountsAction::Move(false));
                }
                KeyCode::Char('K') => self.accounts_action(AccountsAction::Move(true)),
                KeyCode::Char('J') => self.accounts_action(AccountsAction::Move(false)),
                KeyCode::Up | KeyCode::Char('k') => {
                    dialog.cursor = dialog.cursor.checked_sub(1).unwrap_or(count - 1);
                }
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    dialog.cursor = (dialog.cursor + 1) % count;
                }
                KeyCode::Char('a') => self.accounts_action(AccountsAction::Add),
                KeyCode::Char('r') | KeyCode::Enter => self.accounts_action(AccountsAction::Rename),
                KeyCode::Char('d') | KeyCode::Delete => {
                    self.accounts_action(AccountsAction::Remove);
                }
                _ => {}
            },
            AccountsMode::Rename(input) => match key.code {
                KeyCode::Esc => self.accounts_action(AccountsAction::Back),
                KeyCode::Enter => self.accounts_action(AccountsAction::Confirm),
                _ => edit(input, key),
            },
            AccountsMode::Add(form) => match key.code {
                KeyCode::Esc => self.accounts_action(AccountsAction::Back),
                _ if form.busy => {}
                KeyCode::Enter => self.accounts_action(AccountsAction::Confirm),
                KeyCode::Up | KeyCode::BackTab => {
                    form.field = form.field.checked_sub(1).unwrap_or(ADD_FIELDS.len() - 1);
                }
                KeyCode::Down | KeyCode::Tab => form.field = (form.field + 1) % ADD_FIELDS.len(),
                _ => edit(&mut form.fields[form.field], key),
            },
            AccountsMode::Remove => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.accounts_action(AccountsAction::Confirm);
                }
                _ => self.accounts_action(AccountsAction::Back),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::Event;

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

    fn two_accounts() -> App {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        let mut work = app.state.accounts[0].clone();
        work.profile = "work".into();
        work.label = "work".into();
        work.user_id = "@me:work".into();
        app.state.accounts.push(work);
        app.on_update(Update::Rooms(vec![
            RoomEntry {
                id: "!a".into(),
                name: "Lunch".into(),
                ..RoomEntry::default()
            },
            RoomEntry {
                id: "work/!b".into(),
                name: "Standup".into(),
                account: 1,
                ..RoomEntry::default()
            },
        ]));
        app
    }

    #[test]
    fn profile_names_come_from_the_server() {
        assert_eq!(crate::accounts::profile_from_server("matrix.org"), "matrix");
        assert_eq!(
            crate::accounts::profile_from_server("https://Chat.Example.org:8448"),
            "chat"
        );
        assert_eq!(
            crate::accounts::profile_from_server("@amy:school.example"),
            "school"
        );
        assert_eq!(
            crate::accounts::profile_from_server("http://127.0.0.1:8008"),
            "127"
        );
    }

    #[test]
    fn the_settings_row_opens_the_dialog_and_r_renames() {
        let mut app = two_accounts();
        app.overlays.open(SettingsDialog {
            cursor: super::super::Setting::Accounts.index(),
            edit: None,
        });
        app.on_event(key(KeyCode::Enter));
        assert!(!app.overlays.is_open::<SettingsDialog>());
        assert!(app.overlays.is_open::<AccountsDialog>());

        app.on_event(key(KeyCode::Down));
        app.on_event(key(KeyCode::Char('r')));
        app.on_event(Event::Key(KeyEvent::new(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
        )));
        type_text(&mut app, "Office");
        app.on_event(key(KeyCode::Enter));
        assert_eq!(app.state.accounts[1].label, "Office");
        assert_eq!(app.state.accounts[1].profile, "work");
        assert!(app.state.config_dirty);
    }

    #[test]
    fn adding_asks_the_main_loop_to_sign_in_and_reports_back() {
        let mut app = two_accounts();
        app.open_accounts();
        app.on_event(key(KeyCode::Char('a')));
        // Without a server there is nothing to ask.
        app.on_event(key(KeyCode::Enter));
        assert_eq!(app.take_account_request(), None);

        type_text(&mut app, "work.example.org");
        app.on_event(key(KeyCode::Tab));
        type_text(&mut app, "amy");
        app.on_event(key(KeyCode::Tab));
        type_text(&mut app, "secret");
        app.on_event(key(KeyCode::Enter));
        // "work" is taken, so the name made from the server gets a number.
        assert_eq!(
            app.take_account_request(),
            Some(AccountRequest::Add(crate::accounts::SignIn {
                profile: "work2".into(),
                label: String::new(),
                server: "work.example.org".into(),
                user: "amy".into(),
                password: "secret".into(),
            }))
        );

        app.account_added(Err("wrong password".into()));
        let Some(AccountsDialog {
            mode: AccountsMode::Add(form),
            ..
        }) = app.overlays.get::<AccountsDialog>()
        else {
            panic!("the form stays open after a failure");
        };
        assert!(form.failed && !form.busy);
        assert_eq!(form.status, "wrong password");
        assert!(form.fields[PASSWORD].is_empty());
    }

    #[test]
    fn signing_out_asks_first_and_takes_the_rooms_along() {
        let mut app = two_accounts();
        app.state.account_filter = Some(1);
        // Opens on the account being shown.
        app.open_accounts();
        app.on_event(key(KeyCode::Char('d')));
        assert_eq!(app.take_account_request(), None);
        app.on_event(key(KeyCode::Char('y')));
        assert_eq!(app.take_account_request(), Some(AccountRequest::Remove(1)));

        let closed = app.state.forget_account(1);
        app.account_gone(closed);
        assert_eq!(app.state.accounts.len(), 1);
        assert_eq!(app.state.account_filter, None);
        assert_eq!(app.state.rooms.len(), 1);
        assert_eq!(app.state.rooms[0].name, "Lunch");
    }

    #[test]
    fn accounts_can_be_moved_up_and_down() {
        let mut app = two_accounts();
        app.state.account_filter = Some(1);
        app.open_accounts();
        // Opens on "work" (the one shown); move it to the top.
        app.on_event(key(KeyCode::Char('K')));
        assert_eq!(app.take_account_request(), Some(AccountRequest::Swap(1, 0)));
        app.state.swap_accounts(1, 0);
        assert_eq!(app.state.accounts[0].profile, "work");
        assert_eq!(app.state.account_filter, Some(0));
        assert_eq!(app.state.rooms[0].name, "Standup");
        assert_eq!(app.state.rooms[0].account, 0);
        assert_eq!(app.overlays.get::<AccountsDialog>().unwrap().cursor, 0);
        // Already on top: nothing to do.
        app.on_event(key(KeyCode::Char('K')));
        assert_eq!(app.take_account_request(), None);
    }

    #[test]
    fn the_only_account_cannot_be_signed_out_here() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.open_accounts();
        app.on_event(key(KeyCode::Char('d')));
        app.on_event(key(KeyCode::Char('y')));
        assert_eq!(app.take_account_request(), None);
        assert!(app.notice.is_some());
    }
}
