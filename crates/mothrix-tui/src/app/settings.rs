// SPDX-License-Identifier: MPL-2.0

//! The settings dialog: its rows, what each shows, and how it changes.
//!
//! A new setting is a new [`Setting`] variant, put into [`Setting::ALL`]
//! where it should show; the compiler then asks for its label, value and
//! change. Saving it is `config.rs`.

use super::*;

/// One row of the settings dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Theme,
    RoomOrder,
    Pictures,
    Notifications,
    SpellCheck,
    RoomEvents,
    MixedList,
    LongMessages,
    LanguageToolServer,
    LanguageToolLanguages,
    LanguageToolUser,
    LanguageToolKey,
    Accounts,
    Spaces,
    TypingNotices,
    ReadReceipts,
    LinkPreviews,
    Proxy,
    Updates,
}

const LONG_MESSAGE_LIMITS: [usize; 4] = [0, 8, 15, 30];

const IMAGE_MODES: [&str; 5] = ["auto", "kitty", "sixel", "blocks", "off"];

impl Setting {
    /// The rows in display order.
    pub const ALL: [Setting; 19] = [
        Self::Theme,
        Self::RoomOrder,
        Self::Pictures,
        Self::Notifications,
        Self::SpellCheck,
        Self::RoomEvents,
        Self::MixedList,
        Self::LongMessages,
        Self::LanguageToolServer,
        Self::LanguageToolLanguages,
        Self::LanguageToolUser,
        Self::LanguageToolKey,
        Self::Accounts,
        Self::Spaces,
        Self::TypingNotices,
        Self::ReadReceipts,
        Self::LinkPreviews,
        Self::Proxy,
        Self::Updates,
    ];

    /// The row at `index` in display order (the last for any beyond).
    pub fn at(index: usize) -> Self {
        Self::ALL[index.min(Self::ALL.len() - 1)]
    }

    /// Its position in display order.
    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|setting| *setting == self)
            .unwrap_or(0)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Theme => "Theme",
            Self::RoomOrder => "Room order",
            Self::Pictures => "Pictures",
            Self::Notifications => "Notifications",
            Self::SpellCheck => "Spell check",
            Self::RoomEvents => "Join and leave messages",
            Self::MixedList => "Rooms and people",
            Self::LongMessages => "Long messages",
            Self::LanguageToolServer => "LanguageTool server",
            Self::LanguageToolLanguages => "LanguageTool languages",
            Self::LanguageToolUser => "LanguageTool user",
            Self::LanguageToolKey => "LanguageTool API key",
            Self::Accounts => "Accounts",
            Self::Spaces => "Spaces",
            Self::TypingNotices => "Typing notices",
            Self::ReadReceipts => "Read receipts",
            Self::LinkPreviews => "Link previews",
            Self::Proxy => "Proxy",
            Self::Updates => "Updates",
        }
    }

    /// What to know before changing it, shown while the cursor is on it.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Self::LanguageToolServer => {
                Some(" What you write is sent to this server to be checked. Empty: off.")
            }
            Self::LanguageToolLanguages => {
                Some(" Like: de, en. Other languages are then never assumed. Empty: any.")
            }
            Self::LanguageToolUser => {
                Some(" Only for the paid service (api.languagetoolplus.com): your e-mail.")
            }
            Self::LanguageToolKey => {
                Some(" Only for the paid service: the API key from your account.")
            }
            Self::Accounts => Some(" Add an account, give one a name of its own, or sign one out."),
            Self::Proxy => Some(
                " Only where there is no direct way out, like http://proxy:3128. From the next start.",
            ),
            Self::LinkPreviews => Some(
                " Your server fetches the pages, so it learns the links (also from encrypted rooms).",
            ),
            Self::Updates => {
                Some(" GitHub sees where you ask from. /update installs a new release.")
            }
            Self::Theme
            | Self::RoomOrder
            | Self::Pictures
            | Self::Notifications
            | Self::SpellCheck
            | Self::RoomEvents
            | Self::MixedList
            | Self::LongMessages
            | Self::Spaces
            | Self::TypingNotices
            | Self::ReadReceipts => None,
        }
    }
}

fn on_off(value: bool) -> String {
    if value { "on" } else { "off" }.to_owned()
}

/// The next of `choices` after `current` (before it, going back), round.
fn step<T: PartialEq + Copy>(choices: &[T], current: T, forward: bool) -> T {
    let count = choices.len();
    let at = choices
        .iter()
        .position(|choice| *choice == current)
        .unwrap_or(0);
    choices[if forward { at + 1 } else { at + count - 1 } % count]
}

impl App {
    /// What a row of the settings dialog shows as its value.
    pub fn setting_value(&self, setting: Setting) -> String {
        let shown = |on: bool, yes: &str, no: &str| if on { yes } else { no }.to_owned();
        match setting {
            Setting::Theme => self.theme.name.to_owned(),
            Setting::RoomOrder => self.state.prefs.sort.label().to_owned(),
            Setting::Pictures => match (self.state.prefs.images.as_str(), self.graphics) {
                ("auto", Graphics::Kitty) => "auto (sharp, kitty graphics)".to_owned(),
                ("auto", Graphics::Sixel) => "auto (sharp, sixel graphics)".to_owned(),
                ("auto", _) => "auto (blocks)".to_owned(),
                (other, _) => other.to_owned(),
            },
            Setting::Notifications => on_off(self.state.prefs.notifications),
            Setting::SpellCheck => {
                match (self.state.prefs.spellcheck, self.spell_languages.is_empty()) {
                    (false, _) => "off".to_owned(),
                    (true, true) => "on (no dictionary found)".to_owned(),
                    (true, false) => format!("on ({})", self.spell_languages.join(", ")),
                }
            }
            Setting::RoomEvents => shown(self.state.prefs.room_events, "shown", "hidden"),
            Setting::MixedList => {
                shown(self.state.prefs.mixed_list, "one list", "separate sections")
            }
            Setting::LongMessages if self.state.prefs.long_messages == 0 => {
                "shown in full".to_owned()
            }
            Setting::LongMessages => {
                format!("folded after {} lines", self.state.prefs.long_messages)
            }
            Setting::LanguageToolServer if self.state.prefs.languagetool_url.is_empty() => {
                "not set".to_owned()
            }
            Setting::LanguageToolServer => self.state.prefs.languagetool_url.clone(),
            Setting::LanguageToolLanguages
                if self.state.prefs.languagetool_languages.is_empty() =>
            {
                "any (detected)".to_owned()
            }
            Setting::LanguageToolLanguages => self.state.prefs.languagetool_languages.join(", "),
            Setting::LanguageToolUser if self.state.prefs.languagetool_username.is_empty() => {
                "none (not needed)".to_owned()
            }
            Setting::LanguageToolUser => self.state.prefs.languagetool_username.clone(),
            Setting::LanguageToolKey if self.state.prefs.languagetool_api_key.is_empty() => {
                "none (not needed)".to_owned()
            }
            // Never shown again once entered.
            Setting::LanguageToolKey => "set (hidden)".to_owned(),
            Setting::Proxy if self.state.prefs.proxy.is_empty() => "none (direct)".to_owned(),
            Setting::Proxy => self.state.prefs.proxy.clone(),
            Setting::Accounts => {
                let names: Vec<&str> = self
                    .state
                    .accounts
                    .iter()
                    .map(|account| account.label.as_str())
                    .collect();
                format!("{} · Enter to manage", names.join(", "))
            }
            Setting::Spaces => shown(self.state.prefs.spaces, "group the room list", "not used"),
            Setting::TypingNotices => shown(
                self.state.prefs.typing_notices,
                "others see when you write",
                "not sent",
            ),
            Setting::ReadReceipts => shown(self.state.prefs.read_receipts, "shown", "hidden"),
            Setting::LinkPreviews => shown(self.state.prefs.link_previews, "shown", "off"),
            Setting::Updates => self.updates_setting(),
        }
    }

    /// Change a setting to its next value (its previous one, going back).
    /// Settings that are typed open a text field instead.
    pub(super) fn change_setting(&mut self, setting: Setting, forward: bool) {
        if let Some(settings) = self.overlays.get_mut::<SettingsDialog>() {
            settings.edit = None;
        }
        match setting {
            Setting::Theme => {
                let themes = theme::all();
                let names: Vec<&str> = themes.iter().map(|theme| theme.name).collect();
                let next = step(&names, self.theme.name, forward);
                self.theme = themes[names.iter().position(|name| *name == next).unwrap_or(0)];
            }
            Setting::RoomOrder => {
                // Three modes: going back is going forward twice.
                self.state.prefs.sort = if forward {
                    self.state.prefs.sort.next()
                } else {
                    self.state.prefs.sort.next().next()
                };
                self.sidebar_scroll = 0;
            }
            Setting::Pictures => {
                let current = IMAGE_MODES
                    .iter()
                    .copied()
                    .find(|mode| *mode == self.state.prefs.images)
                    .unwrap_or(IMAGE_MODES[0]);
                self.state.prefs.images = step(&IMAGE_MODES, current, forward).to_owned();
                self.graphics = Graphics::from_label(
                    &self.state.prefs.images,
                    self.kitty_supported,
                    self.sixel_supported,
                );
                self.notice =
                    Some("The picture setting applies to pictures loaded from now on.".to_owned());
            }
            Setting::Notifications => {
                self.state.prefs.notifications = !self.state.prefs.notifications
            }
            Setting::SpellCheck => {
                self.state.prefs.spellcheck = !self.state.prefs.spellcheck;
                self.spell_key = None;
                self.refresh_spelling();
            }
            Setting::RoomEvents => self.state.prefs.room_events = !self.state.prefs.room_events,
            Setting::MixedList => self.toggle_mixed_list(),
            Setting::LongMessages => {
                self.state.prefs.long_messages = step(
                    &LONG_MESSAGE_LIMITS,
                    self.state.prefs.long_messages,
                    forward,
                );
            }
            Setting::Accounts => return self.open_accounts(),
            Setting::LinkPreviews => {
                self.state.prefs.link_previews = !self.state.prefs.link_previews;
                self.ask_link_previews();
            }
            Setting::TypingNotices => {
                self.state.prefs.typing_notices = !self.state.prefs.typing_notices
            }
            Setting::ReadReceipts => {
                self.state.prefs.read_receipts = !self.state.prefs.read_receipts
            }
            Setting::Spaces => {
                self.state.prefs.spaces = !self.state.prefs.spaces;
                self.sidebar_scroll = 0;
            }
            Setting::Updates => self.state.prefs.updates = !self.state.prefs.updates,
            Setting::LanguageToolServer
            | Setting::LanguageToolLanguages
            | Setting::LanguageToolUser
            | Setting::LanguageToolKey
            | Setting::Proxy => {
                let mut input = Input::default();
                match setting {
                    Setting::Proxy => input.set(&self.state.prefs.proxy),
                    Setting::LanguageToolLanguages => {
                        input.set(&self.state.prefs.languagetool_languages.join(", "))
                    }
                    Setting::LanguageToolUser => input.set(&self.state.prefs.languagetool_username),
                    // The key is typed anew rather than shown.
                    Setting::LanguageToolKey => {}
                    _ => input.set(&self.state.prefs.languagetool_url),
                }
                if let Some(settings) = self.overlays.get_mut::<SettingsDialog>() {
                    settings.edit = Some(input);
                }
                return;
            }
        }
        self.state.config_dirty = true;
    }

    /// Take what was typed for a setting.
    fn set_typed_setting(&mut self, setting: Setting, typed: String) {
        if setting == Setting::Proxy {
            self.state.prefs.proxy = typed;
            self.notice = Some("The proxy is used from the next start.".to_owned());
            self.state.config_dirty = true;
            return;
        }
        let mut url = self.state.prefs.languagetool_url.clone();
        match setting {
            Setting::LanguageToolLanguages => {
                let variants = crate::languagetool::preferred_variants(&self.spell_languages);
                self.state.prefs.languagetool_languages =
                    crate::languagetool::parse_languages(&typed, &variants);
            }
            Setting::LanguageToolUser => self.state.prefs.languagetool_username = typed,
            Setting::LanguageToolKey => self.state.prefs.languagetool_api_key = typed,
            _ => url = typed,
        }
        // Check again with what changed.
        self.set_languagetool_url(url);
        self.state.config_dirty = true;
    }

    pub(super) fn on_settings_key(&mut self, key: KeyEvent) {
        let Some(settings) = self.overlays.get_mut::<SettingsDialog>() else {
            return;
        };
        let setting = Setting::at(settings.cursor);
        if let Some(input) = settings.edit.as_mut() {
            match key.code {
                KeyCode::Esc => settings.edit = None,
                KeyCode::Enter => {
                    let typed = input.text().trim().to_owned();
                    settings.edit = None;
                    self.set_typed_setting(setting, typed);
                }
                _ => {
                    input.edit(key);
                }
            }
            return;
        }
        let count = Setting::ALL.len();
        match key.code {
            KeyCode::Esc | KeyCode::F(2) | KeyCode::Char('q') => {
                self.overlays.close::<SettingsDialog>()
            }
            KeyCode::Up | KeyCode::Char('k') => {
                settings.cursor = (settings.cursor + count - 1) % count;
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                settings.cursor = (settings.cursor + 1) % count;
            }
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Char('l') => {
                self.change_setting(setting, true);
            }
            KeyCode::Left | KeyCode::Char('h') => self.change_setting(setting, false),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn every_row_shows_a_value_and_enter_changes_it() {
        for setting in Setting::ALL {
            let mut app = App::new(theme::THEMES[0], "@me:x".into());
            app.overlays.open(SettingsDialog {
                cursor: setting.index(),
                edit: None,
            });
            let before = app.setting_value(setting);
            assert!(!before.is_empty(), "{setting:?}");
            app.on_event(key(KeyCode::Enter));
            match setting {
                Setting::Accounts => assert!(app.overlays.is_open::<AccountsDialog>()),
                Setting::LanguageToolServer
                | Setting::LanguageToolLanguages
                | Setting::LanguageToolUser
                | Setting::LanguageToolKey
                | Setting::Proxy => {
                    let settings = app.overlays.get::<SettingsDialog>();
                    assert!(settings.is_some_and(|s| s.edit.is_some()), "{setting:?}");
                }
                _ => {
                    assert!(app.state.config_dirty, "{setting:?}");
                    assert_ne!(app.setting_value(setting), before, "{setting:?}");
                    // Left goes back to where it was.
                    app.on_event(key(KeyCode::Left));
                    assert_eq!(app.setting_value(setting), before, "{setting:?}");
                }
            }
        }
    }

    #[test]
    fn the_cursor_goes_round() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.on_event(key(KeyCode::F(2)));
        app.on_event(key(KeyCode::Up));
        let cursor = app.overlays.get::<SettingsDialog>().map(|s| s.cursor);
        assert_eq!(cursor, Some(Setting::Updates.index()));
        app.on_event(key(KeyCode::Down));
        let cursor = app.overlays.get::<SettingsDialog>().map(|s| s.cursor);
        assert_eq!(cursor, Some(Setting::Theme.index()));
    }

    #[test]
    fn a_proxy_is_typed_in_and_saved() {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.overlays.open(SettingsDialog {
            cursor: Setting::Proxy.index(),
            edit: None,
        });
        assert_eq!(app.setting_value(Setting::Proxy), "none (direct)");
        app.on_event(key(KeyCode::Enter));
        for c in "http://p:3128".chars() {
            app.on_event(key(KeyCode::Char(c)));
        }
        app.on_event(key(KeyCode::Enter));
        assert_eq!(app.state.prefs.proxy, "http://p:3128");
        assert!(app.state.config_dirty);
    }
}
