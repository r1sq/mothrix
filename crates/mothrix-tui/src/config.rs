// SPDX-License-Identifier: MPL-2.0

//! How the config file (`mothrix_core::config`)
//! maps onto the settings of [`App`] and back, and the terminal's own
//! themes in it.
//!
//! A new preference is a field of `Prefs` (in mothrix-core) with its
//! default; its row in the settings dialog is `app::Setting`.

use crate::{
    app::{App, Graphics},
    theme,
};
pub use mothrix_core::config::Config;

/// What the terminal does with the config, beyond what `mothrix-core` does.
pub trait ConfigExt {
    fn apply(&self, app: &mut App, dictionaries: &[String]);
    fn updated(&self, app: &App) -> Config;
    fn custom_themes(&self) -> (Vec<theme::Theme>, Vec<String>);
}

impl ConfigExt for Config {
    /// Put the settings into `app`, whose accounts and terminal abilities
    /// are known already (and own themes registered). `dictionaries` are
    /// the spelling languages in use.
    fn apply(&self, app: &mut App, dictionaries: &[String]) {
        app.theme = theme::by_name(&self.theme).unwrap_or(theme::THEMES[0]);
        self.name_accounts(&mut app.state.accounts);
        app.state.account_filter = self.account_filter(&app.state.accounts);
        app.state.prefs = self.prefs.tidied(dictionaries);
        app.graphics = Graphics::from_label(
            &app.state.prefs.images,
            app.kitty_supported,
            app.sixel_supported,
        );
        app.state.sections = crate::sections::OwnSections::new(self.sections.clone());
        app.state.folded = crate::sidebar::Folds::new(self.folded.iter().cloned());
    }

    /// This config with the settings as they are now in `app`. What only
    /// the file holds stays: key bindings, own themes, dictionaries, and
    /// what is known of accounts that are not running (`--profile`).
    fn updated(&self, app: &App) -> Config {
        Config {
            theme: app.theme.name.to_owned(),
            ..self.with_shown(app.state.shown())
        }
    }

    /// The own themes of the config, and what could not be read of them.
    fn custom_themes(&self) -> (Vec<theme::Theme>, Vec<String>) {
        let (themes, problems) = mothrix_core::theme::own(&self.themes);
        (themes.iter().map(theme::Theme::from).collect(), problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Account;
    use mothrix_core::prefs::Prefs;

    fn app_with(config: &Config, profiles: &[&str]) -> App {
        let mut app = App::new(theme::THEMES[0], "@me:x".into());
        app.state.accounts = profiles
            .iter()
            .map(|profile| Account {
                profile: (*profile).to_owned(),
                label: (*profile).to_owned(),
                ..app.state.accounts[0].clone()
            })
            .collect();
        config.apply(&mut app, &[]);
        app
    }

    fn text(config: &Config) -> String {
        toml::to_string_pretty(config).unwrap()
    }

    #[test]
    fn a_fresh_app_has_the_defaults_of_an_empty_file() {
        let app = App::new(theme::THEMES[0], "@me:x".into());
        assert_eq!(app.state.prefs, Prefs::default());
        assert_eq!(app.state.prefs.images, "auto");
    }

    #[test]
    fn without_a_file_the_switches_are_on() {
        let app = app_with(&Config::default(), &["default"]);
        assert!(
            app.state.prefs.notifications
                && app.state.prefs.spellcheck
                && app.state.prefs.room_events
        );
        assert!(
            app.state.prefs.spaces
                && app.state.prefs.typing_notices
                && app.state.prefs.read_receipts
        );
        assert!(!app.state.prefs.link_previews && !app.state.prefs.mixed_list);
        assert_eq!(app.state.prefs.long_messages, 15);
    }

    #[test]
    fn saving_keeps_what_only_the_file_holds() {
        let config: Config = toml::from_str(
            r##"
            theme = "tokyo-night"
            sort = "name"
            images = "blocks"
            spell_languages = ["de_DE"]
            account = "work"
            account_order = ["work", "default"]
            folded = ["PEOPLE"]
            [account_names]
            default = "Private"
            school = "School"
            [keys]
            search = "ctrl+s"
            [[themes]]
            name = "mine"
            accent = "#ff0000"
            "##,
        )
        .unwrap();
        // Run with one account only (`--profile default`).
        let app = app_with(&config, &["default"]);
        assert_eq!(app.state.accounts[0].label, "Private");
        assert_eq!(text(&config.updated(&app)), text(&config));
    }

    #[test]
    fn saving_takes_what_changed() {
        let config = Config::default();
        let mut app = app_with(&config, &["default", "work"]);
        app.state.prefs.mixed_list = true;
        app.state.accounts[1].label = "Job".into();
        app.state.account_filter = Some(1);
        app.state.folded.toggle("work:CHATS");
        let saved = config.updated(&app);
        assert!(saved.prefs.mixed_list);
        assert_eq!(saved.account, "work");
        assert_eq!(saved.account_order, ["default", "work"]);
        assert_eq!(
            saved.account_names.get("work").map(String::as_str),
            Some("Job")
        );
        assert_eq!(saved.folded, ["work:CHATS"]);
        // And reads back the same.
        let again = app_with(&saved, &["default", "work"]);
        assert_eq!(text(&saved.updated(&again)), text(&saved));
    }
}
